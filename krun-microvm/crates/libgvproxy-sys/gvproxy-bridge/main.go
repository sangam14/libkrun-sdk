package main

/*
#include <stdlib.h>

typedef void (*log_callback_fn)(int level, const char* message);

static void call_rust_log_callback(void* callback, int level, const char* msg) {
	if (callback != NULL) {
		((log_callback_fn)callback)(level, msg);
	}
}
*/
import "C"
import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"sync"
	"unsafe"

	"github.com/containers/gvisor-tap-vsock/pkg/types"
	"github.com/containers/gvisor-tap-vsock/pkg/virtualnetwork"
	logrus "github.com/sirupsen/logrus"
)

// Log level constants matching Rust tracing
const (
	LogLevelTrace = 0
	LogLevelDebug = 1
	LogLevelInfo  = 2
	LogLevelWarn  = 3
	LogLevelError = 4
)

var (
	rustLogCallback unsafe.Pointer
	callbackMu      sync.RWMutex
)

// RustTracingLogrusHook forwards logrus logs to Rust tracing
type RustTracingLogrusHook struct{}

func (h *RustTracingLogrusHook) Levels() []logrus.Level {
	return logrus.AllLevels
}

func (h *RustTracingLogrusHook) Fire(entry *logrus.Entry) error {
	callbackMu.RLock()
	cb := rustLogCallback
	callbackMu.RUnlock()

	if cb == nil {
		return nil
	}

	buf := []byte(entry.Message)
	for k, v := range entry.Data {
		buf = append(buf, ' ')
		buf = append(buf, k...)
		buf = append(buf, '=')
		buf = append(buf, fmt.Sprint(v)...)
	}

	var rustLevel int
	switch entry.Level {
	case logrus.TraceLevel:
		rustLevel = LogLevelTrace
	case logrus.DebugLevel:
		rustLevel = LogLevelDebug
	case logrus.InfoLevel:
		rustLevel = LogLevelInfo
	case logrus.WarnLevel:
		rustLevel = LogLevelWarn
	default:
		rustLevel = LogLevelError
	}

	cMsg := C.CString(string(buf))
	C.call_rust_log_callback(cb, C.int(rustLevel), cMsg)
	C.free(unsafe.Pointer(cMsg))
	return nil
}

//export gvproxy_set_log_callback
func gvproxy_set_log_callback(callback unsafe.Pointer) {
	callbackMu.Lock()
	rustLogCallback = callback
	callbackMu.Unlock()

	if callback != nil {
		logrus.SetLevel(logrus.DebugLevel)
		logrus.SetOutput(io.Discard)
		logrus.AddHook(&RustTracingLogrusHook{})
	} else {
		logrus.SetOutput(os.Stderr)
	}
}

// DNSRecord represents an A record
type DNSRecord struct {
	Name string `json:"name"`
	IP   string `json:"ip"`
}

// DNSZone represents a local DNS zone
type DNSZone struct {
	Name      string      `json:"name"`
	Records   []DNSRecord `json:"records,omitempty"`
	DefaultIP string      `json:"default_ip"`
}

// GvproxyConfig matches the configuration struct expected from Rust
type GvproxyConfig struct {
	SocketPath        string            `json:"socket_path"`
	ControlSocketPath string            `json:"control_socket_path,omitempty"`
	Subnet            string            `json:"subnet"`
	GatewayIP         string            `json:"gateway_ip"`
	GatewayMac        string            `json:"gateway_mac"`
	GuestIP           string            `json:"guest_ip"`
	GuestMac          string            `json:"guest_mac"`
	MTU               int               `json:"mtu"`
	DNSZones          []DNSZone         `json:"dns_zones,omitempty"`
	DNSSearchDomains  []string          `json:"dns_search_domains,omitempty"`
	Debug             bool              `json:"debug"`
	Forwards          map[string]string `json:"forwards,omitempty"`
	AllowNet          []string          `json:"allow_net,omitempty"`
}

// GvproxyInstance tracks a running virtual network instance
type GvproxyInstance struct {
	ID         int64
	SocketPath string
	GuestIP    string
	Cancel     context.CancelFunc
	Listener   net.Listener
	VN         *virtualnetwork.VirtualNetwork
}

var (
	instancesMu sync.RWMutex
	instances   = make(map[int64]*GvproxyInstance)
	nextID      int64 = 1
)

func setErr(errOut **C.char, err error) {
	if errOut != nil && err != nil {
		*errOut = C.CString(err.Error())
	}
}

//export gvproxy_create
func gvproxy_create(configJSON *C.char, errOut **C.char) C.longlong {
	if configJSON == nil {
		setErr(errOut, fmt.Errorf("configJSON cannot be null"))
		return -1
	}

	rawJSON := C.GoString(configJSON)
	var cfg GvproxyConfig
	if err := json.Unmarshal([]byte(rawJSON), &cfg); err != nil {
		setErr(errOut, fmt.Errorf("invalid json config: %w", err))
		return -1
	}

	if cfg.SocketPath == "" {
		setErr(errOut, fmt.Errorf("socket_path is required"))
		return -1
	}

	if cfg.Subnet == "" {
		cfg.Subnet = "192.168.127.0/24"
	}
	if cfg.GatewayIP == "" {
		cfg.GatewayIP = "192.168.127.1"
	}
	if cfg.GatewayMac == "" {
		cfg.GatewayMac = "5a:94:ef:e4:0c:dd"
	}
	if cfg.GuestIP == "" {
		cfg.GuestIP = "192.168.127.2"
	}
	if cfg.GuestMac == "" {
		cfg.GuestMac = "5a:94:ef:e4:0c:ee"
	}
	if cfg.MTU <= 0 {
		cfg.MTU = 1500
	}

	// Prepare DNS Zones
	dnsZones := make([]types.Zone, 0, len(cfg.DNSZones))
	for _, z := range cfg.DNSZones {
		zone := types.Zone{
			Name:      z.Name,
			DefaultIP: net.ParseIP(z.DefaultIP),
		}
		for _, r := range z.Records {
			zone.Records = append(zone.Records, types.Record{
				Name: r.Name,
				IP:   net.ParseIP(r.IP),
			})
		}
		dnsZones = append(dnsZones, zone)
	}

	natMap := map[string]string{
		cfg.GatewayIP: "127.0.0.1",
	}

	forwards := cfg.Forwards
	if forwards == nil {
		forwards = make(map[string]string)
	}

	typesConfig := &types.Configuration{
		Debug:             cfg.Debug,
		MTU:               cfg.MTU,
		Subnet:            cfg.Subnet,
		GatewayIP:         cfg.GatewayIP,
		GatewayMacAddress: cfg.GatewayMac,
		DHCPStaticLeases: map[string]string{
			cfg.GuestIP: cfg.GuestMac,
		},
		Forwards:          forwards,
		NAT:               natMap,
		GatewayVirtualIPs: []string{cfg.GatewayIP},
		Protocol:          types.QemuProtocol,
		DNS:               dnsZones,
		DNSSearchDomains:  cfg.DNSSearchDomains,
	}

	vn, err := virtualnetwork.New(typesConfig)
	if err != nil {
		setErr(errOut, fmt.Errorf("failed to create virtual network: %w", err))
		return -1
	}

	// Remove any pre-existing socket file
	_ = os.Remove(cfg.SocketPath)

	listener, err := net.Listen("unix", cfg.SocketPath)
	if err != nil {
		setErr(errOut, fmt.Errorf("failed to listen on socket %s: %w", cfg.SocketPath, err))
		return -1
	}

	ctx, cancel := context.WithCancel(context.Background())

	// Spawn Qemu stream accept loop in background goroutine
	go func() {
		for {
			conn, err := listener.Accept()
			if err != nil {
				select {
				case <-ctx.Done():
					return
				default:
					logrus.WithError(err).Warn("gvproxy listener accept error")
					return
				}
			}

			go func(c net.Conn) {
				if err := vn.AcceptQemu(ctx, c); err != nil && err != io.EOF {
					select {
					case <-ctx.Done():
					default:
						logrus.WithError(err).Warn("gvproxy AcceptQemu connection error")
					}
				}
			}(conn)
		}
	}()

	instancesMu.Lock()
	id := nextID
	nextID++
	instances[id] = &GvproxyInstance{
		ID:         id,
		SocketPath: cfg.SocketPath,
		GuestIP:    cfg.GuestIP,
		Cancel:     cancel,
		Listener:   listener,
		VN:         vn,
	}
	instancesMu.Unlock()

	return C.longlong(id)
}

//export gvproxy_expose_port
func gvproxy_expose_port(id C.longlong, localPort C.int, remotePort C.int, errOut **C.char) C.int {
	instancesMu.RLock()
	inst, ok := instances[int64(id)]
	instancesMu.RUnlock()

	if !ok {
		setErr(errOut, fmt.Errorf("instance id %d not found", id))
		return -1
	}

	local := fmt.Sprintf("0.0.0.0:%d", localPort)
	remote := fmt.Sprintf("%s:%d", inst.GuestIP, remotePort)

	payload, _ := json.Marshal(map[string]string{
		"local":  local,
		"remote": remote,
	})

	req := httptest.NewRequest("POST", "/services/forwarder/expose", bytes.NewReader(payload))
	w := httptest.NewRecorder()
	inst.VN.Mux().ServeHTTP(w, req)

	if w.Code != http.StatusOK {
		setErr(errOut, fmt.Errorf("expose port failed (%d): %s", w.Code, w.Body.String()))
		return -1
	}

	return 0
}

//export gvproxy_unexpose_port
func gvproxy_unexpose_port(id C.longlong, localPort C.int, errOut **C.char) C.int {
	instancesMu.RLock()
	inst, ok := instances[int64(id)]
	instancesMu.RUnlock()

	if !ok {
		setErr(errOut, fmt.Errorf("instance id %d not found", id))
		return -1
	}

	local := fmt.Sprintf("0.0.0.0:%d", localPort)
	payload, _ := json.Marshal(map[string]string{
		"local": local,
	})

	req := httptest.NewRequest("POST", "/services/forwarder/unexpose", bytes.NewReader(payload))
	w := httptest.NewRecorder()
	inst.VN.Mux().ServeHTTP(w, req)

	if w.Code != http.StatusOK {
		setErr(errOut, fmt.Errorf("unexpose port failed (%d): %s", w.Code, w.Body.String()))
		return -1
	}

	return 0
}

//export gvproxy_destroy
func gvproxy_destroy(id C.longlong) C.int {
	instancesMu.Lock()
	inst, ok := instances[int64(id)]
	if ok {
		delete(instances, int64(id))
	}
	instancesMu.Unlock()

	if !ok {
		return 0
	}

	inst.Cancel()
	if inst.Listener != nil {
		_ = inst.Listener.Close()
	}
	_ = os.Remove(inst.SocketPath)

	return 0
}

//export gvproxy_free_string
func gvproxy_free_string(str *C.char) {
	if str != nil {
		C.free(unsafe.Pointer(str))
	}
}

func main() {}

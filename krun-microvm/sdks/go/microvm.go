// Package microvm provides an idiomatic Go SDK for controlling libkrun microVMs.
package microvm

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"os/exec"
	"strconv"
	"strings"
)

// Config defines the launch parameters for a microVM.
type Config struct {
	Image      string
	CPUs       uint8
	MemoryMb   uint32
	Ports      []string
	Volumes    []string
	Env        map[string]string
	Workdir    string
	NoNetwork  bool
	Kernel     string
	Initrd     string
	Cmdline    string
	Firmware   string
	Disks      []string
	AllowHosts []string
	Secrets    map[string]string
	MaxTokens  uint64
	Cmd        []string
}

// ExecResult contains the output and exit status of a command executed in a microVM.
type ExecResult struct {
	ExitCode int
	Stdout   string
	Stderr   string
}

// VM represents an active microVM instance.
type VM struct {
	ID string
}

func getBinary() string {
	if bin := os.Getenv("MICROVM_BIN"); bin != "" {
		return bin
	}
	return "microvm"
}

// BuildRunArgs constructs the CLI argument vector from a Config struct.
func BuildRunArgs(cfg Config) []string {
	args := []string{"run", "-d"}

	if cfg.CPUs > 0 {
		args = append(args, "-c", strconv.Itoa(int(cfg.CPUs)))
	}
	if cfg.MemoryMb > 0 {
		args = append(args, "-m", strconv.Itoa(int(cfg.MemoryMb)))
	}
	if cfg.Workdir != "" {
		args = append(args, "-w", cfg.Workdir)
	}
	if cfg.NoNetwork {
		args = append(args, "--no-network")
	}
	for _, p := range cfg.Ports {
		args = append(args, "-p", p)
	}
	for _, v := range cfg.Volumes {
		args = append(args, "-v", v)
	}
	for k, v := range cfg.Env {
		args = append(args, "-e", fmt.Sprintf("%s=%s", k, v))
	}
	if cfg.Kernel != "" {
		args = append(args, "--kernel", cfg.Kernel)
	}
	if cfg.Initrd != "" {
		args = append(args, "--initrd", cfg.Initrd)
	}
	if cfg.Cmdline != "" {
		args = append(args, "--cmdline", cfg.Cmdline)
	}
	if cfg.Firmware != "" {
		args = append(args, "--firmware", cfg.Firmware)
	}
	for _, d := range cfg.Disks {
		args = append(args, "--disk", d)
	}
	for _, h := range cfg.AllowHosts {
		args = append(args, "--allow-host", h)
	}
	for k, v := range cfg.Secrets {
		args = append(args, "--secret", fmt.Sprintf("%s=%s", k, v))
	}
	if cfg.MaxTokens > 0 {
		args = append(args, "--max-tokens", strconv.FormatUint(cfg.MaxTokens, 10))
	}
	if cfg.Image != "" {
		args = append(args, cfg.Image)
	}
	if len(cfg.Cmd) > 0 {
		args = append(args, "--")
		args = append(args, cfg.Cmd...)
	}
	return args
}

// Run spawns a new hardware-isolated microVM.
func Run(ctx context.Context, cfg Config) (*VM, error) {
	bin := getBinary()
	args := BuildRunArgs(cfg)

	cmd := exec.CommandContext(ctx, bin, args...)
	out, err := cmd.Output()
	if err != nil {
		return nil, fmt.Errorf("failed to run microvm: %w: %s", err, string(out))
	}

	lines := strings.Split(strings.TrimSpace(string(out)), "\n")
	id := lines[len(lines)-1]
	if id == "" {
		return nil, fmt.Errorf("empty VM id returned")
	}

	return &VM{ID: id}, nil
}

// Exec runs a command inside the microVM.
func (v *VM) Exec(ctx context.Context, cmdArgs []string, env map[string]string) (*ExecResult, error) {
	bin := getBinary()
	args := []string{"exec"}

	for k, val := range env {
		args = append(args, "-e", fmt.Sprintf("%s=%s", k, val))
	}
	args = append(args, v.ID, "--")
	args = append(args, cmdArgs...)

	cmd := exec.CommandContext(ctx, bin, args...)
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr

	err := cmd.Run()
	exitCode := 0
	if err != nil {
		if exitErr, ok := err.(*exec.ExitError); ok {
			exitCode = exitErr.ExitCode()
		} else {
			return nil, err
		}
	}

	return &ExecResult{
		ExitCode: exitCode,
		Stdout:   stdout.String(),
		Stderr:   stderr.String(),
	}, nil
}

// Pause pauses all vCPUs in the microVM.
func (v *VM) Pause(ctx context.Context) error {
	cmd := exec.CommandContext(ctx, getBinary(), "pause", v.ID)
	return cmd.Run()
}

// Resume unfreezes a paused microVM.
func (v *VM) Resume(ctx context.Context) error {
	cmd := exec.CommandContext(ctx, getBinary(), "resume", v.ID)
	return cmd.Run()
}

// Stop terminates the microVM.
func (v *VM) Stop(ctx context.Context) error {
	cmd := exec.CommandContext(ctx, getBinary(), "stop", v.ID)
	return cmd.Run()
}

// Rm removes the stopped microVM instance.
func (v *VM) Rm(ctx context.Context, force bool) error {
	args := []string{"rm"}
	if force {
		args = append(args, "-f")
	}
	args = append(args, v.ID)
	cmd := exec.CommandContext(ctx, getBinary(), args...)
	return cmd.Run()
}

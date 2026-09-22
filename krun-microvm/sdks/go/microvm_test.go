package microvm

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestBuildRunArgs_Minimal(t *testing.T) {
	cfg := Config{
		Image:    "alpine:latest",
		CPUs:     2,
		MemoryMb: 512,
	}

	args := BuildRunArgs(cfg)
	expected := []string{"run", "-d", "-c", "2", "-m", "512", "alpine:latest"}

	if len(args) != len(expected) {
		t.Fatalf("expected %d args, got %d: %v", len(expected), len(args), args)
	}
	for i, arg := range expected {
		if args[i] != arg {
			t.Errorf("arg[%d]: expected %q, got %q", i, arg, args[i])
		}
	}
}

func TestBuildRunArgs_Comprehensive(t *testing.T) {
	cfg := Config{
		Image:     "python:3.11-slim",
		CPUs:      4,
		MemoryMb:  2048,
		Workdir:   "/workspace",
		NoNetwork: true,
		Ports:     []string{"8080:80", "9000:9000"},
		Volumes:   []string{"/host/data:data:ro"},
		Env: map[string]string{
			"APP_ENV": "production",
		},
		AllowHosts: []string{"api.anthropic.com:443"},
		Secrets: map[string]string{
			"API_KEY": "test-key-123",
		},
		MaxTokens: 50000,
		Cmd:       []string{"python", "main.py"},
	}

	args := BuildRunArgs(cfg)
	argStr := strings.Join(args, " ")

	checks := []string{
		"run -d",
		"-c 4",
		"-m 2048",
		"-w /workspace",
		"--no-network",
		"-p 8080:80",
		"-p 9000:9000",
		"-v /host/data:data:ro",
		"-e APP_ENV=production",
		"--allow-host api.anthropic.com:443",
		"--secret API_KEY=test-key-123",
		"--max-tokens 50000",
		"python:3.11-slim",
		"-- python main.py",
	}

	for _, check := range checks {
		if !strings.Contains(argStr, check) {
			t.Errorf("expected args to contain %q, but got:\n%s", check, argStr)
		}
	}
}

func TestBuildRunArgs_MultiBoot(t *testing.T) {
	cfg := Config{
		Kernel:   "/boot/vmlinuz",
		Initrd:   "/boot/initrd.img",
		Cmdline:  "console=ttyS0 quiet",
		Firmware: "/boot/OVMF.fd",
		Disks:    []string{"/dev/vda:/disk.raw"},
	}

	args := BuildRunArgs(cfg)
	argStr := strings.Join(args, " ")

	if !strings.Contains(argStr, "--kernel /boot/vmlinuz") {
		t.Errorf("expected --kernel, got: %s", argStr)
	}
	if !strings.Contains(argStr, "--initrd /boot/initrd.img") {
		t.Errorf("expected --initrd, got: %s", argStr)
	}
	if !strings.Contains(argStr, "--cmdline console=ttyS0 quiet") {
		t.Errorf("expected --cmdline, got: %s", argStr)
	}
	if !strings.Contains(argStr, "--firmware /boot/OVMF.fd") {
		t.Errorf("expected --firmware, got: %s", argStr)
	}
	if !strings.Contains(argStr, "--disk /dev/vda:/disk.raw") {
		t.Errorf("expected --disk, got: %s", argStr)
	}
}

func TestRun_WithMockBinary(t *testing.T) {
	// Create a temporary mock binary script that prints an ID
	tmpDir := t.TempDir()
	mockBin := filepath.Join(tmpDir, "mock_microvm.sh")
	script := fmt.Sprintf("#!/bin/sh\necho \"vm-mock-12345\"\n")
	if err := os.WriteFile(mockBin, []byte(script), 0755); err != nil {
		t.Fatalf("failed to create mock binary: %v", err)
	}

	t.Setenv("MICROVM_BIN", mockBin)

	vm, err := Run(context.Background(), Config{
		Image:    "alpine:latest",
		CPUs:     1,
		MemoryMb: 256,
	})
	if err != nil {
		t.Fatalf("Run() failed with mock binary: %v", err)
	}
	if vm.ID != "vm-mock-12345" {
		t.Fatalf("expected VM ID 'vm-mock-12345', got %q", vm.ID)
	}
}

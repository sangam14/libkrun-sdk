# krun-sdk-go

Idiomatic Go SDK for launching and orchestrating hardware-isolated microVMs with `libkrun` on macOS Apple Silicon and Linux KVM.

## Installation

```bash
go get github.com/sangam14/libkrun-sdk/sdk/go
```

## Quick Start

```go
package main

import (
	"context"
	"fmt"
	"log"

	"github.com/sangam14/libkrun-sdk/sdk/go"
)

func main() {
	ctx := context.Background()

	// 1. Launch a hardware-isolated microVM
	vm, err := microvm.Run(ctx, microvm.Config{
		Image:    "alpine:latest",
		CPUs:     2,
		MemoryMb: 512,
		AllowHosts: []string{"api.openai.com:443"},
	})
	if err != nil {
		log.Fatalf("Failed to run microvm: %v", err)
	}
	defer vm.Stop(ctx)

	fmt.Printf("Running microVM: %s\n", vm.ID)

	// 2. Execute process inside guest
	res, err := vm.Exec(ctx, []string{"uname", "-a"}, nil)
	if err != nil {
		log.Fatalf("Exec failed: %v", err)
	}
	fmt.Printf("Guest Kernel: %s\n", res.Stdout)
}
```

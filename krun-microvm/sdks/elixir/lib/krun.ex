# Copyright 2026, libkrun-sdk authors.
# SPDX-License-Identifier: Apache-2.0

defmodule Krun do
  @moduledoc """
  Official Elixir SDK for `libkrun-sdk`.

  Provides:
  - Hardware-isolated unstructured document intake & partitioning (`Krun.Unstructured`)
  - AI Agent threat detection (prompt injection, zero-width steganography, SSRF)
  - MicroVM sandboxing with Apple Silicon Hypervisor / Linux KVM
  - Cloudflare Pingora L7 zero-trust egress shielding
  - Dedicated Web UI served at `/unstructured`
  """

  @doc """
  Partitions a document (PDF, DOCX, Markdown, Text, JSON) into structured element schemas.

  ## Examples

      iex> Krun.partition("# Executive Brief\\nKey metrics indicate 30% growth.")
      {:ok, [
        %{type: "Header", text: "Executive Brief", metadata: %{element_id: 1, filename: "document.txt"}},
        %{type: "NarrativeText", text: "Key metrics indicate 30% growth.", metadata: %{element_id: 2, filename: "document.txt"}}
      ]}
  """
  defdelegate partition(document, opts \\ []), to: Krun.Unstructured

  @doc """
  Scans an unstructured document for AI Agent threats (prompt injections, SSRF beacons, zero-width characters).
  """
  defdelegate scan_threats(document, opts \\ []), to: Krun.Unstructured

  @doc """
  Executes hardware-isolated document intake and detonation inside an ephemeral microVM sandbox.
  """
  defdelegate detonate(document, opts \\ []), to: Krun.Unstructured

  @doc """
  Runs a command inside a hardware-isolated libkrun microVM.
  """
  def run_microvm(image, cmd \\ [], opts \\ []) do
    cpus = Keyword.get(opts, :cpus, 2)
    memory = Keyword.get(opts, :memory, 512)
    volumes = Keyword.get(opts, :volumes, [])
    allow_hosts = Keyword.get(opts, :allow_hosts, [])

    args = [
      "run",
      "-c", to_string(cpus),
      "-m", to_string(memory)
    ]

    args =
      Enum.reduce(volumes, args, fn v, acc ->
        acc ++ ["-v", v]
      end)

    args =
      Enum.reduce(allow_hosts, args, fn h, acc ->
        acc ++ ["--allow-host", h]
      end)

    args = args ++ [image]

    args =
      if length(cmd) > 0 do
        args ++ ["--"] ++ cmd
      else
        args
      end

    System.cmd("microvm", args, stderr_to_stdout: true)
  end
end

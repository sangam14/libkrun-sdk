# Copyright 2026, libkrun-sdk authors.
# SPDX-License-Identifier: Apache-2.0

defmodule Krun.Application do
  @moduledoc false
  use Application

  @impl true
  def start(_type, _args) do
    if Application.get_env(:krun, :auto_start_server, true) and Mix.env() != :test do
      port =
        case System.get_env("PORT") do
          nil -> 4000
          val -> String.to_integer(val)
        end

      children = [
        {Krun.Unstructured.Server, [port: port]}
      ]

      opts = [strategy: :one_for_one, name: Krun.Supervisor]
      Supervisor.start_link(children, opts)
    else
      opts = [strategy: :one_for_one, name: Krun.Supervisor]
      Supervisor.start_link([], opts)
    end
  end
end

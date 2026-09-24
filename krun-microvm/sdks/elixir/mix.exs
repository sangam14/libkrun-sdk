defmodule Krun.MixProject do
  use Mix.Project

  def project do
    [
      app: :krun,
      version: "0.1.0",
      elixir: "~> 1.14",
      start_permanent: Mix.env() == :prod,
      description: "Elixir SDK and Unstructured Document Intake & AI Threat Shield for libkrun microVMs",
      package: package(),
      deps: deps()
    ]
  end

  def application do
    [
      extra_applications: [:logger, :inets],
      mod: {Krun.Application, []}
    ]
  end

  defp deps do
    [
      # Standard library JSON is built into Elixir 1.18+ / OTP 27+
    ]
  end

  defp package do
    [
      name: "krun",
      licenses: ["Apache-2.0"],
      links: %{"GitHub" => "https://github.com/containers/libkrun"}
    ]
  end
end

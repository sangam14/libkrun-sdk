# Copyright 2026, libkrun-sdk authors.
# SPDX-License-Identifier: Apache-2.0

defmodule Krun.Unstructured.Server do
  @moduledoc """
  High-performance concurrent Elixir OTP HTTP Server for Unstructured Document Intake,
  serving the Cyber-Obsidian Web UI at `/unstructured` and JSON REST API endpoints.
  """

  use GenServer
  require Logger

  @default_port 4000

  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @impl true
  def init(opts) do
    port = Keyword.get(opts, :port, @default_port)
    
    # Locate WebUI assets
    webui_dir =
      Keyword.get(opts, :webui_dir) ||
        find_webui_dir()

    case :gen_tcp.listen(port, [:binary, packet: :raw, active: false, reuseaddr: true]) do
      {:ok, listen_socket} ->
        Logger.info("[Krun.Unstructured] HTTP Server listening on http://127.0.0.1:#{port}/unstructured")
        # Spawn acceptor loop
        spawn_link(fn -> acceptor_loop(listen_socket, webui_dir) end)
        {:ok, %{listen_socket: listen_socket, port: port, webui_dir: webui_dir}}

      {:error, reason} ->
        Logger.error("[Krun.Unstructured] Failed to bind port #{port}: #{inspect(reason)}")
        {:stop, reason}
    end
  end

  defp find_webui_dir do
    candidates = [
      Path.expand("../../../../webui", __DIR__),
      Path.expand("../../webui", __DIR__),
      "/Users/apple/libkrun-sdk/webui"
    ]

    Enum.find(candidates, &File.dir?/1) || "/Users/apple/libkrun-sdk/webui"
  end

  defp acceptor_loop(listen_socket, webui_dir) do
    case :gen_tcp.accept(listen_socket) do
      {:ok, client_socket} ->
        spawn(fn -> handle_client(client_socket, webui_dir) end)
        acceptor_loop(listen_socket, webui_dir)

      {:error, :closed} ->
        :ok

      {:error, reason} ->
        Logger.warning("[Krun.Unstructured] Accept error: #{inspect(reason)}")
        acceptor_loop(listen_socket, webui_dir)
    end
  end

  defp handle_client(socket, webui_dir) do
    case :gen_tcp.recv(socket, 0, 5000) do
      {:ok, data} ->
        {method, path, headers, body} = parse_http_request(data)
        response = route(method, path, headers, body, webui_dir)
        :gen_tcp.send(socket, response)
        :gen_tcp.close(socket)

      {:error, _} ->
        :gen_tcp.close(socket)
    end
  end

  defp parse_http_request(data) do
    case String.split(data, "\r\n\r\n", parts: 2) do
      [header_part, body] ->
        [request_line | header_lines] = String.split(header_part, "\r\n")
        [method, path | _] = String.split(request_line, " ")

        headers =
          Enum.reduce(header_lines, %{}, fn line, acc ->
            case String.split(line, ": ", parts: 2) do
              [k, v] -> Map.put(acc, String.downcase(k), v)
              _ -> acc
            end
          end)

        {method, path, headers, body}

      _ ->
        {"GET", "/", %{}, ""}
    end
  end

  # =========================================================================
  # HTTP Routing & Handlers
  # =========================================================================

  # Root redirect to /unstructured
  defp route("GET", "/", _headers, _body, _webui_dir) do
    redirect_response("/unstructured")
  end

  # /unstructured: Main Cyber-Obsidian Web UI
  defp route("GET", path, _headers, _body, webui_dir) when path in ["/unstructured", "/unstructured/"] do
    index_file = Path.join(webui_dir, "index.html")

    case File.read(index_file) do
      {:ok, html} ->
        html_response(200, html)

      {:error, _} ->
        text_response(404, "WebUI index.html not found in #{webui_dir}")
    end
  end

  # CSS Asset
  defp route("GET", path, _headers, _body, webui_dir)
       when path in ["/index.css", "/unstructured/index.css"] do
    css_file = Path.join(webui_dir, "index.css")

    case File.read(css_file) do
      {:ok, css} ->
        file_response(200, "text/css; charset=utf-8", css)

      {:error, _} ->
        text_response(404, "CSS asset not found")
    end
  end

  # JS Asset
  defp route("GET", path, _headers, _body, webui_dir)
       when path in ["/app.js", "/unstructured/app.js"] do
    js_file = Path.join(webui_dir, "app.js")

    case File.read(js_file) do
      {:ok, js} ->
        file_response(200, "application/javascript; charset=utf-8", js)

      {:error, _} ->
        text_response(404, "JS asset not found")
    end
  end

  # API: Health check
  defp route("GET", "/api/unstructured/health", _headers, _body, _webui_dir) do
    json_response(200, %{
      status: "ok",
      engine: "libkrun-sdk",
      hypervisor: "Apple Silicon HVF / Linux KVM",
      egress_proxy: "Cloudflare Pingora 0.9.0 L7",
      page_path: "/unstructured",
      timestamp: System.system_time(:second)
    })
  end

  # API: Document Partitioning
  defp route("POST", "/api/unstructured/partition", _headers, body, _webui_dir) do
    case parse_json(body) do
      {:ok, params} ->
        doc = params["document"] || params["text"] || ""
        filename = params["filename"] || "document.txt"

        {:ok, elements} = Krun.Unstructured.partition(doc, filename: filename)

        json_response(200, %{
          status: "ok",
          filename: filename,
          elements_count: length(elements),
          elements: elements
        })

      {:error, _} ->
        json_response(400, %{status: "error", message: "Invalid JSON body"})
    end
  end

  # API: AI Threat Scan
  defp route("POST", "/api/unstructured/scan", _headers, body, _webui_dir) do
    case parse_json(body) do
      {:ok, params} ->
        doc = params["document"] || params["text"] || ""
        filename = params["filename"] || "document.txt"

        {:ok, result} = Krun.Unstructured.scan_threats(doc, filename: filename)
        json_response(200, %{status: "ok", result: result})

      {:error, _} ->
        json_response(400, %{status: "error", message: "Invalid JSON body"})
    end
  end

  # API: MicroVM Detonation & Sanitization
  defp route("POST", "/api/unstructured/detonate", _headers, body, _webui_dir) do
    case parse_json(body) do
      {:ok, params} ->
        doc = params["document"] || params["text"] || ""
        filename = params["filename"] || "document.pdf"

        {:ok, report} = Krun.Unstructured.detonate(doc, filename: filename)
        json_response(200, %{status: "ok", report: report})

      {:error, _} ->
        json_response(400, %{status: "error", message: "Invalid JSON body"})
    end
  end

  # 404 Not Found fallback
  defp route(_method, path, _headers, _body, _webui_dir) do
    json_response(404, %{
      error: "NotFound",
      message: "Path '#{path}' not found. Did you mean '/unstructured'?",
      hint: "Visit http://127.0.0.1:4000/unstructured"
    })
  end

  # =========================================================================
  # Response Builders
  # =========================================================================

  defp redirect_response(location) do
    """
    HTTP/1.1 302 Found\r
    Location: #{location}\r
    Content-Length: 0\r
    Connection: close\r
    \r
    """
  end

  defp html_response(code, html) do
    bytes = byte_size(html)

    """
    HTTP/1.1 #{code} OK\r
    Content-Type: text/html; charset=utf-8\r
    Content-Length: #{bytes}\r
    X-Powered-By: libkrun-elixir\r
    Connection: close\r
    \r
    #{html}
    """
  end

  defp file_response(code, content_type, body) do
    bytes = byte_size(body)

    """
    HTTP/1.1 #{code} OK\r
    Content-Type: #{content_type}\r
    Content-Length: #{bytes}\r
    X-Powered-By: libkrun-elixir\r
    Connection: close\r
    \r
    #{body}
    """
  end

  defp text_response(code, text) do
    bytes = byte_size(text)

    """
    HTTP/1.1 #{code} Error\r
    Content-Type: text/plain; charset=utf-8\r
    Content-Length: #{bytes}\r
    Connection: close\r
    \r
    #{text}
    """
  end

  defp json_response(code, data) do
    json = JSON.encode!(data)
    bytes = byte_size(json)

    """
    HTTP/1.1 #{code} OK\r
    Content-Type: application/json; charset=utf-8\r
    Content-Length: #{bytes}\r
    X-Powered-By: libkrun-elixir\r
    Connection: close\r
    \r
    #{json}
    """
  end

  defp parse_json(body) do
    try do
      {:ok, JSON.decode!(body)}
    rescue
      _ -> {:error, :invalid_json}
    end
  end
end

{
  runCommand,
  curl,
  python3,
  package,
}:
runCommand "rlmonitor-package-check"
  {
    nativeBuildInputs = [
      curl
      python3
    ];
  }
  ''
    set -euo pipefail
    mkdir work
    cd work
    cat > .env <<ENV
    REALITYLINK_ADDRESS=127.0.0.1:1
    LISTEN_ADDR=127.0.0.1:18080
    DATABASE_PATH=history.db
    SERVER_NAME=Nix-package-test
    RUST_LOG=off
    ENV

    ${package}/bin/rlmonitor >server.log 2>&1 &
    server_pid=$!
    trap 'kill "$server_pid" 2>/dev/null || true; wait "$server_pid" 2>/dev/null || true' EXIT

    ready=false
    for attempt in {1..100}; do
      if curl --fail --silent http://127.0.0.1:18080/health >health; then
        ready=true
        break
      fi
      sleep 0.1
    done
    if [ "$ready" != true ]; then cat server.log; exit 1; fi
    grep -Fx OK health
    curl --fail --silent http://127.0.0.1:18080/ >index.html
    curl --fail --silent http://127.0.0.1:18080/api/overview >overview.json
    python3 - <<'PY'
    import json, pathlib, re, urllib.request
    overview = json.loads(pathlib.Path('overview.json').read_text())
    assert overview['server_name'] == 'Nix-package-test'
    assert overview['realitylink_address'] == '127.0.0.1:1'
    assert pathlib.Path('history.db').is_file()
    html = pathlib.Path('index.html').read_text()
    assert '<title>RLMonitor' in html
    assets = re.findall(r'(?:src|href)="(/assets/[^\"]+)"', html)
    assert len(assets) >= 2
    for asset in assets:
        with urllib.request.urlopen('http://127.0.0.1:18080' + asset) as response:
            assert response.read(), asset
    PY

    kill "$server_pid"
    wait "$server_pid"
    trap - EXIT

    mkdir custom
    echo custom-frontend > custom/index.html
    echo FRONTEND_DIR=custom >> .env
    ${package}/bin/rlmonitor >server.log 2>&1 &
    server_pid=$!
    trap 'kill "$server_pid" 2>/dev/null || true; wait "$server_pid" 2>/dev/null || true' EXIT
    for attempt in {1..100}; do
      if curl --fail --silent http://127.0.0.1:18080/ >custom.html && grep -Fx custom-frontend custom.html; then
        touch "$out"
        exit 0
      fi
      sleep 0.1
    done
    cat server.log
    exit 1
  ''

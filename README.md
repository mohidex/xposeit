# XposeIt
Instantly expose your local apps

---

## Build

```bash
cargo build --release
```

Binaries: `target/release/server` and `target/release/client`.

---

## Run Server

```bash
./target/release/server --http-addr 0.0.0.0:8080 --tcp-addr 0.0.0.0:8081
```

- `--http-addr`: HTTP server address (default: `0.0.0.0:8080`).
- `--tcp-addr`: TCP server address (default: `0.0.0.0:8081`).

---

## Run Client

```bash
./target/release/client --server-addr 127.0.0.1:8081 --subdomain myapp --local-addr 127.0.0.1:3000
```

- `--server-addr`: Server address (default: `127.0.0.1:8081`).
- `--subdomain`: Subdomain for the tunnel (required).
- `--local-addr`: Local service address (default: `127.0.0.1:3000`).

---

## Access Tunnel

Visit:

```
http://myapp.your-server.com
```

Replace `your-server.com` with your server's domain or IP.

---

## Troubleshooting

- Ensure ports are open and not in use.
- Verify server and client are running.
- Check subdomain registration.

---

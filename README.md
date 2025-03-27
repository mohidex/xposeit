# XposeIt
Instantly expose your local apps

## Quick Start


1. **Build the binaries**:
   ```bash
   cargo build
   ```

2. **Run server and Expose a port**:
   ```bash
   # Run server
   ./target/debug/xpose-server
   # Expose a local app
   ./target/debug/xpose-cli --port 8000
   ```
   Output:
   ```
   Listening at localhost:22475
   Your localhost:8000 server is accessible through localhost:22475
   ```

3. **Access your server**:
   Open `http://localhost:22475` in your browser.

---

## Install

1. Install Rust: [rustup.rs](https://rustup.rs/).
2. Clone and build:
   ```bash
   git clone https://github.com/mohidex/xposeit.git
   cd xposeit
   cargo build --release
   ```

---

Happy exposing! 🚀

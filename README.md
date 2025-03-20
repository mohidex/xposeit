# XposeIt
Instantly expose your local apps

## Quick Start

1. **Start the server**:
   ```bash
   cargo run server
   ```

2. **Expose a port**:
   ```bash
   cargo run xpose -p 8000
   ```
   Output:
   ```
   Listening at localhost:22475
   Your server is accessible through localhost:22475
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

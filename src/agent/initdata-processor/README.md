# InitData Processor

## Overview

`initdata-processor` is a component of Kata Containers that processes initdata (initialization data) during virtual machine startup. It is responsible for:

1. Locating and identifying measured and unmeasured initdata devices
2. Reading and decompressing initdata content
3. Validating the structural integrity of initdata
4. Writing measured configuration files to the `/run/measured-cfg` directory
5. Writing unmeasured configuration files to the `/run/unmeasured-cfg` directory

Measured and unmeasured initdata use the same simple block image layout:

```text
offset 0..7    8-byte magic
offset 8..15   gzip payload length as little-endian uint64
offset 16..    gzip-compressed TOML payload
```

The magic value identifies the payload type:

```text
initdata    measured initdata
unmsrdat    unmeasured initdata
```

Unmeasured initdata is not included in the initdata digest or TEE launch measurement. It is intended for untrusted boot-time configuration hints.

## Build and Installation

### Build Requirements

- Rust toolchain (1.92 or higher)
- Make

### Build Commands

```bash
# Default build (release mode)
make

# Or explicitly specify
make build

# Run tests
make test

# Code format check
make fmt

# Run clippy static analysis
make clippy

# Run all checks
make check

# Clean build artifacts
make clean
```

### Installation

```bash
# Install to default location (/usr)
sudo make install

# Specify installation prefix
sudo make install PREFIX=/usr/local

# Specify systemd directory
sudo make install SYSTEMD_DIR=/etc/systemd/system

# Uninstall
sudo make uninstall
```

Installed file locations:

- Binary: `/usr/bin/initdata-processor`
- Systemd service: `/usr/lib/systemd/system/initdata-processor.service`

## Usage

### Command Line Arguments

```bash
initdata-processor [OPTIONS]

OPTIONS:
    --config-path <PATH>                Measured configuration output directory (default: /run/measured-cfg)
    --unmeasured-config-path <PATH>     Unmeasured configuration output directory (default: /run/unmeasured-cfg)
    --dev-path <PATH>                   Device search directory (default: /dev)
```

### Examples

```bash
# Use default configuration
/usr/bin/initdata-processor

# Specify custom configuration path
/usr/bin/initdata-processor --config-path /custom/config/path

# Specify custom unmeasured configuration path
/usr/bin/initdata-processor --unmeasured-config-path /custom/unmeasured/config/path

# Specify custom device path
/usr/bin/initdata-processor --dev-path /custom/dev/path
```

## Payload Format

Measured and unmeasured initdata are TOML documents with file content stored under the `[data]` table. Unmeasured initdata does not carry a digest algorithm.

Example unmeasured initdata:

```toml
version = "0.1.0"

[data]
"aa.toml" = '''
customer_id = "example"
'''

"cdh.toml" = '''
region = "us-east-1"
'''
```

The processor writes those entries as files:

```text
/run/unmeasured-cfg/aa.toml
/run/unmeasured-cfg/cdh.toml
```

Kata agent launches guest components with unmeasured AA/CDH config files when present. If they are absent, it falls back to the measured initdata config files.

## Systemd Service Management

### Enable and Start Service

```bash
# Reload systemd configuration
sudo systemctl daemon-reload

# Enable service (start on boot)
sudo systemctl enable initdata-processor.service

# Start service
sudo systemctl start initdata-processor.service

# Check service status
sudo systemctl status initdata-processor.service
```

### Service Configuration Details

The systemd service file is located at `/usr/lib/systemd/system/initdata-processor.service`:

```ini
[Unit]
Description=Kata InitData Processor
Documentation=https://github.com/kata-containers/kata-containers

[Service]
Type=oneshot
ExecStart=/usr/bin/initdata-processor
RemainAfterExit=yes
StandardOutput=journal
StandardError=journal
Restart=no

[Install]
WantedBy=multi-user.target
```

**Service Type Explanation:**

- `Type=oneshot`: Service executes once and exits, suitable for one-time initialization tasks
- `RemainAfterExit=yes`: Systemd considers the service active even after the process exits
- `Restart=no`: No automatic restart, as this is a one-time initialization task

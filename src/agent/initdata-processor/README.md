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

Unmeasured initdata is disabled by default. Measured initdata must include
`unmeasured-initdata-cfg.toml` to enable it:

```toml
version = "0.1.0"
algorithm = "sha256"

[data]
"unmeasured-initdata-cfg.toml" = '''
enabled = true
allowed_keys = ["aa.toml"]
'''
```

If this file is absent, or if `enabled` is `false`, the processor ignores the
unmeasured initdata device. `allowed_keys` is an exact, case-sensitive allowlist
controlled by measured initdata. The `allowed_keys` field is required and must
contain at least one valid file name when `enabled = true`. When disabled, use
`enabled = false` with `allowed_keys = []`. Unknown fields, incorrect field types,
and invalid file names are rejected when the measured policy is parsed, even
if no unmeasured device is present.
An enabled payload containing any unlisted key is rejected before any unmeasured
files (including the raw document) are written. Keys must be plain file names;
paths and processor-reserved names are rejected even if listed. Existing measured
configurations must add `allowed_keys` to continue accepting unmeasured entries.

When `enabled = true`, the unmeasured initdata device is required. If it is
missing, the processor returns an error before writing unmeasured files.

Example unmeasured initdata:

```toml
version = "0.1.0"

[data]
"aa.toml" = '''
customer_id = "example"
'''
```

The processor writes extracted entries beneath `initdata/`. The complete raw
document remains at the output root as `initdata.toml`:

```text
/run/unmeasured-cfg/initdata.toml
/run/unmeasured-cfg/initdata/aa.toml
```

The Kata agent launches AA with measured configuration and an optional extracted
unmeasured `aa.toml`. CDH receives its measured configuration.

## Systemd Service Management

### Enable and Start Service

`make install` enables the service under `kata-containers.target` for systemd
guests. The kata-agent service requires the processor to complete successfully
before it starts and launches guest components. Systemd creates both output
directories, and the processor leaves them empty when no initdata is present
on a fresh boot.

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
WantedBy=kata-containers.target
```

**Service Type Explanation:**

- `Type=oneshot`: Service executes once and exits, suitable for one-time initialization tasks
- `RemainAfterExit=yes`: Systemd considers the service active even after the process exits
- `Restart=no`: No automatic restart, as this is a one-time initialization task

### Captured measured data and attestation

The processor writes the exact decompressed measured document to
`/run/measured-cfg/initdata.toml` before enforcing the unmeasured allowlist.
When the processor is installed, kata-agent reads this capture for its digest,
measured configuration, and the document passed to AA. A host changing the block
device after extraction cannot substitute a different policy for verification.
An absent capture represents absent measured initdata and does not trigger a
second device read. Images without the processor retain direct device loading.

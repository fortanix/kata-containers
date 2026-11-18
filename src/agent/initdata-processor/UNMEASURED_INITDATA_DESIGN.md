<!--
Copyright (c) 2026 Fortanix, Inc.

SPDX-License-Identifier: Apache-2.0
-->

# Unmeasured Initdata Design

## Summary

Unmeasured initdata provides a controlled way to deliver per-launch,
host-supplied configuration into a confidential Kata guest without including
that configuration in the trusted initdata digest. It is carried on a second
block device, separate from measured initdata.

Measured initdata remains the trust anchor. A file inside measured initdata,
`unmeasured-initdata-cfg.toml`, decides whether the runtime and guest may
consume the unmeasured document. The feature is disabled when that file is
absent or contains `enabled = false`.

The first supported consumer is Attestation Agent (AA). Unmeasured input is
not passed to Confidential Data Hub (CDH).

The architecture and boot example are illustrated in
[ARCHITECTURE.md](ARCHITECTURE.md).

## Motivation

Some configuration must vary between launches but must not change an already
calculated confidential-guest image measurement. One example is a CCM
AppConfig ID that selects application-specific context during attestation.

Putting such a value in measured initdata would change the measured initdata
digest for every value. Passing it after application startup was also not an
acceptable delivery model. A separate boot-time block device provides the
required lifecycle while keeping the value outside the measured initdata
digest.

This mechanism does not make host data trustworthy. It makes the trust
boundary explicit and lets trusted measured configuration decide whether that
class of input is accepted at all.

## Goals

- Deliver optional host-controlled configuration during VM creation.
- Keep payload changes out of the measured initdata digest.
- Require an explicit opt-in from measured initdata.
- Keep measured and unmeasured documents separate throughout the flow.
- Process both documents before kata-agent starts.
- Expose unmeasured configuration only to components that explicitly support
  it.
- Preserve existing behavior when the feature is not enabled.

## Non-goals

- Providing integrity, authenticity, or confidentiality for unmeasured input.
- Allowing unmeasured input to define policy, trust anchors, or authorization.
- Merging unmeasured values into measured configuration.
- Passing unmeasured configuration to CDH.
- Dynamically changing a process environment after that process starts.
- Removing kata-agent's legacy measured-initdata processing in the current
  phase.

## Trust Model

There are three relevant trust classes:

| Data | Trust property |
|---|---|
| Guest kernel, rootfs, and processor | Covered by the confidential-guest build and launch trust model |
| Measured initdata | Its digest is bound to the platform-specific attestation measurement |
| Unmeasured initdata | Controlled by the host and not bound to the measured initdata digest |

The trusted gate is stored inside measured initdata:

```toml
version = "0.1.0"
algorithm = "sha256"

[data]
"unmeasured-initdata-cfg.toml" = '''
enabled = true
allowed_keys = ["aa.toml"]
'''
```

The measured policy requires both `enabled` and `allowed_keys` and rejects
unknown fields. When enabled, list at least one plain file name, such as
`allowed_keys = ["aa.toml"]`. Every unmeasured entry must match a listed name
exactly; any unlisted entry rejects the whole payload before files are written.
When disabled, use `allowed_keys = []`. Invalid policy fields or file names
fail policy parsing even when no unmeasured device is present.


The runtime checks this gate before accepting the unmeasured annotation. The
guest processor checks the extracted gate again before reading the unmeasured
device. The duplicate checks provide consistent behavior at both sides of the
trust boundary; the guest-side check is authoritative for consumption.

Unmeasured values must never:

- weaken the agent policy;
- replace a verifier, CA, key broker, or trust anchor;
- independently authorize secret release;
- select arbitrary executables or writable host paths;
- override measured security configuration.

## Input Documents

### Measured annotation

The existing annotation carries gzip-compressed, base64-encoded measured
initdata:

```text
io.katacontainers.config.hypervisor.cc_init_data
```

Besides existing measured configuration, its `[data]` table may contain the
trusted `unmeasured-initdata-cfg.toml` gate.

### Unmeasured annotation

The new annotation carries a gzip-compressed, base64-encoded
`UnmeasuredInitData` TOML document:

```text
io.katacontainers.config.hypervisor.cc_unmeasured_init_data
```

The runtime configuration must allow this annotation through
`enable_annotations`. The current format is version `0.1.0`:

```toml
version = "0.1.0"

[data]
"aa.toml" = '''
[ccm_as]
ccm_appconfig_id = "0123456789abcdef"
'''
```

Unlike measured `InitData`, `UnmeasuredInitData` has no digest algorithm field.
Its `data` member remains a `HashMap<String, String>`, matching the measured
initdata file-oriented model.

No explicit payload-size limit has been added. Existing annotation,
orchestrator, API-server, command-line, memory, and block-image limits still
apply indirectly. A deliberate upper bound remains desirable before treating
the interface as generally available.

## Host Runtime Flow

1. Kubernetes/containerd passes the pod annotations to the Kata runtime.
2. Kata decodes and parses measured initdata.
3. Kata reads `unmeasured-initdata-cfg.toml` from the measured document.
4. If the gate is absent or false, Kata ignores the unmeasured annotation and
   leaves `HypervisorConfig.UnmeasuredInitdata` empty.
5. If enabled, Kata verifies that the annotation is allowed, decodes the
   gzip/base64 value, parses `UnmeasuredInitData`, and validates its version.
6. The hypervisor preparation path creates a separate image under:

   ```text
   /run/kata-containers/shared/unmeasured-initdata/<sandbox-id>/data.img
   ```

7. QEMU or Cloud Hypervisor attaches that image as a distinct block device.

The measured and unmeasured block images use the same framing:

```text
+----------------------+----------------------+-----------------------------+
| 8-byte magic         | 8-byte little-endian | gzip-compressed TOML        |
|                      | payload length       | payload                     |
+----------------------+----------------------+-----------------------------+
```

Their identities differ:

| Device | Hypervisor ID | Magic |
|---|---|---|
| Measured initdata | `initdata` | `initdata` |
| Unmeasured initdata | `unmeasured-initdata` | `unmsrdat` |

The distinct ID and magic prevent the processor from confusing the two
devices. Reusing the common framing avoids introducing another image format.

## Platform and Hypervisor Support

The current Go runtime has attachment paths for:

- QEMU architectures using the common QEMU block-device implementation;
- QEMU s390x through its architecture-specific implementation;
- Cloud Hypervisor.

The arm64 and ppc64le QEMU implementations currently log that unmeasured
initdata attachment is not implemented. Those platforms must not be described
as supported until their device paths are implemented and tested.

The payload itself is intentionally excluded from the trusted initdata digest.
Adding initdata-processor to the guest rootfs changes the guest image and
therefore requires a one-time rebuild and new expected image/rootfs
measurement. After that build is measured, changing only the unmeasured
payload does not require rebuilding the guest image.

Whether device topology contributes to a particular TEE launch measurement is
platform and firmware specific. The security claim is limited to the payload
not being incorporated into the measured-initdata digest; it is not a general
claim that every platform measurement remains byte-for-byte unchanged.

## Guest Boot Flow

The systemd-based guest boots with:

```text
systemd.unit=kata-containers.target
```

`kata-containers.target` requires `kata-agent.service`.
`kata-agent.service` in turn contains:

```ini
Requires=initdata-processor.service
After=initdata-processor.service
```

`initdata-processor.service` is a oneshot service. It creates and may write only
these runtime directories:

```text
/run/measured-cfg
/run/unmeasured-cfg
```

The processor performs the following operations:

1. Locate the measured device using `/dev/disk/by-id/virtio-initdata`.
2. If the by-id path is unavailable, scan `vd*` and `sd*` block devices.
3. Require the eight-byte `initdata` magic.
4. Read the declared compressed length and decompress the TOML payload.
5. Parse and structurally validate `InitData`.
6. Write the original document and extracted files under
   `/run/measured-cfg`.
7. Read `/run/measured-cfg/initdata/unmeasured-initdata-cfg.toml`.
8. If enabled, locate the second device using the
   `virtio-unmeasured-initdata` ID and `unmsrdat` magic.
9. Parse and structurally validate `UnmeasuredInitData`.
10. Write the original document and extracted files under
    `/run/unmeasured-cfg`.

Generated layouts are:

```text
/run/measured-cfg/initdata.toml
/run/measured-cfg/initdata/<measured-key>

/run/unmeasured-cfg/initdata.toml
/run/unmeasured-cfg/initdata/<unmeasured-key>
```

Files are created read-only (`0444`). Output directories use mode `0755`.
Before writing a new document, the processor removes stale extracted data but
preserves the top-level systemd-managed runtime directory.

Keys containing unresolved parent paths are rejected by checking that the
canonical parent remains below the intended `initdata` output directory.
Nested directories are not currently supported.

The processor performs format and structural validation. It does not read TEE
registers or independently compare the measured initdata digest with
HOSTDATA/MRCONFIGID. Measurement verification remains part of the platform
attestation and relying-party flow.

## Current Phase and Kata-agent

The implementation is currently in the gated-processing phase:

- initdata-processor completes before kata-agent starts;
- initdata-processor writes the new `/run/measured-cfg` and
  `/run/unmeasured-cfg` layouts;
- kata-agent still reads and processes the measured initdata device using its
  legacy path under `/run/confidential-containers/initdata`;
- kata-agent still obtains policy from that legacy measured processing path;
- kata-agent still launches AA and CDH.

This duplication is intentional during migration. It changes boot ordering
without changing agent policy behavior in the same step.

When unmeasured processing is enabled and AA's extracted file exists,
kata-agent launches AA with:

```text
-u /run/unmeasured-cfg/initdata/aa.toml
```

If the gate is disabled, malformed, or the AA file is absent, kata-agent does
not add `-u`. CDH is launched with measured configuration only.

AA parses the unmeasured file independently from its measured configuration.
For the initial CCM-AS use case, the optional value is:

```toml
[ccm_as]
ccm_appconfig_id = "0123456789abcdef"
```

AA may place that identifier in the CCM attestation/certificate request. The
identifier selects context; it does not itself prove identity or authorize key
release. The attestation service must continue to make authorization decisions
from verified evidence and trusted policy.

## Failure and Disabled Behavior

| Condition | Host behavior | Guest behavior |
|---|---|---|
| No measured annotation/device | No measured image | Processor exits successfully and writes nothing |
| No trusted gate | Ignore unmeasured annotation | Do not read unmeasured device |
| `enabled = false` with `allowed_keys = []` | Ignore unmeasured annotation | Do not read unmeasured device |
| Gate enabled, no unmeasured annotation/device | No unmeasured image | Processor fails because the required unmeasured device is missing |
| Invalid enabled unmeasured annotation | Sandbox configuration fails | VM is not launched with that document |
| Malformed measured device | Not detected by device builder | Processor fails; required kata-agent service does not start |
| Malformed enabled unmeasured device | Not detected by attachment alone | Processor fails; required kata-agent service does not start |

The feature is completely off when the measured gate is absent or false. In
that state, malformed or hostile data in the unmeasured annotation is not
decoded or attached.

## Security Considerations

Unmeasured initdata introduces risks that do not apply to measured initdata:

- **Host substitution:** the host can replace values on every launch.
- **Replay:** the host can replay an older validly formatted document.
- **Equivocation:** different guests can receive different values while
  presenting the same measured initdata digest.
- **Availability attacks:** malformed or expensive input can prevent agent
  startup when the trusted gate is enabled.
- **Semantic injection:** a consumer may accidentally interpret an untrusted
  selector as authorization.
- **Resource exhaustion:** there is currently no dedicated decompressed-size
  limit.

Mitigations in the current design are:

- disabled-by-default behavior;
- opt-in controlled by measured initdata;
- independent device identity and magic;
- schema/version validation;
- path containment checks and read-only output files;
- explicit per-component consumption;
- no merging into measured configuration;
- fail-closed startup after the trusted gate has enabled processing.

Consumers must validate their own fields and treat every unmeasured value as
attacker-controlled. Where freshness or authorization matters, the value must
be bound to verified evidence, a nonce, a signed server-side policy, or another
trusted protocol mechanism.

## Build and Packaging

The processor is built from `src/agent/initdata-processor` and installed into
the systemd-based guest rootfs as:

```text
/usr/bin/initdata-processor
/usr/lib/systemd/system/initdata-processor.service
/etc/systemd/system/kata-containers.target.wants/initdata-processor.service
```

The confidential rootfs build includes the binary and service. Changes to the
runtime annotation, image creation, or block-device attachment require a new
Go shim/runtime build. Changes to AA require rebuilding guest-components and
then rebuilding the rootfs or extension artifact that embeds them.

## Implementation Map

| Responsibility | Source |
|---|---|
| Shared unmeasured document, codec, validation, and trusted gate | `src/libs/kata-types/src/unmeasured_initdata.rs` |
| Annotation name and allowlist handling | `src/libs/kata-types/src/annotations/mod.rs` |
| Go OCI annotation and trusted-gate handling | `src/runtime/pkg/oci/utils.go` |
| Host block-image creation | `src/runtime/virtcontainers/hypervisor.go` |
| QEMU attachment and cleanup | `src/runtime/virtcontainers/qemu.go`, `qemu_arch_base.go`, and architecture implementations |
| Cloud Hypervisor attachment and cleanup | `src/runtime/virtcontainers/clh.go` |
| Guest device discovery and decoding | `src/agent/initdata-processor/src/initdata.rs` |
| Guest processing, gate, and file output | `src/agent/initdata-processor/src/main.rs` |
| Boot ordering and sandboxing | `src/agent/initdata-processor/systemd-services/initdata-processor.service` and `src/agent/kata-agent.service` |
| AA launch argument | `src/agent/src/main.rs` |

## Remaining Work

- Add explicit compressed and decompressed payload-size limits.
- Add complete integration coverage for supported TEEs and hypervisors.
- Decide and document behavior for unsupported arm64 and ppc64le QEMU paths.
- Migrate guest components to systemd services reading `/run/measured-cfg`.
- Migrate kata-agent policy loading to `/run/measured-cfg`.
- Remove kata-agent's duplicate measured-initdata parsing after migration.
- Add external/platform measurement validation tests for each supported TEE.
- Review every future unmeasured consumer as a separate security boundary.


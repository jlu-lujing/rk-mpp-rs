# Security

`rk-mpp-rs` talks directly to the Rockchip media kernel driver through
`/dev/mpp_service` and to dma-heap devices. It is intended to run on trusted
boards with a trusted kernel.

Please report vulnerabilities privately:

- Open a private security advisory on GitHub
  (`Security` -> `Advisories` -> `Report a vulnerability`), or
- Contact the maintainer listed in the repository profile.

Please include the board/SoC, kernel version, driver version, the input that
triggers the issue and whether a kernel log (`dmesg`) shows a reset or timeout.

Known intentional behaviours (for example, refusing streams the C library
mis-handles) are documented in [`docs/DIVERGENCES.md`](docs/DIVERGENCES.md)
and are not security reports on their own.

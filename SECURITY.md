# Security

`rk-mpp-rs` talks directly to the Rockchip media kernel driver through
`/dev/mpp_service` and to dma-heap devices. It is intended to run on trusted
boards with a trusted kernel.

Please report vulnerabilities privately:

- Open a private security advisory on GitHub
  (`Security` -> `Advisories` -> `Report a vulnerability`), or
- Contact the maintainer listed in the repository profile.

Include the SoC, board, kernel and driver version, the triggering stream or
API input, and whether `dmesg` contains a reset, timeout or IOMMU fault.

Known intentional rejections of streams that C MPP mishandles are documented
in [`docs/DIVERGENCES.md`](docs/DIVERGENCES.md) and are not security reports
by themselves.

# t6 sustained CPU frequency cap

The t6 board runs RK3588 with three cpufreq policies:

| policy | CPUs | governor | maximum |
| --- | --- | --- | ---: |
| `policy0` | 0-3, little | `performance` | 1,416,000 kHz |
| `policy4` | 4-5, big | `performance` | 1,800,000 kHz |
| `policy6` | 6-7, big | `performance` | 1,800,000 kHz |

This is a frequency cap, not a calibrated CPU power limit. It lowers the top
OPPs to the highest available step at or below 80% of each cluster's hardware
maximum: 1.416 GHz is 78.7% of the little cluster's 1.8 GHz, and 1.8 GHz is
79.8% of the big clusters' 2.256 GHz. The governors are pinned to `performance`
at the capped OPP so concurrent MPP self-tests do not create unconstrained
frequency/voltage transitions. RK3588 exposes no generic userspace CPU power-cap
interface on this kernel.

The earlier 1.608/1.800 GHz profile was not thermally limited: idle package
readings were 38–39°C. A previous boot ended after a JPEG task spun in the
vendor MPP driver at `mpp_dma_find_buffer_fd()`, producing CPU6 soft-lockup and
RCU stall reports. The lower cap reduces timing/current stress while that
driver defect remains under investigation; it does not by itself fix the
kernel deadlock.

The files are installed as:

```text
/usr/local/sbin/rk3588-cpu-cap
/etc/systemd/system/rk3588-cpu-cap.service
```

The oneshot service is enabled in `multi-user.target`, so every boot reapplies
the caps.

To inspect:

```bash
systemctl status rk3588-cpu-cap.service
for p in /sys/devices/system/cpu/cpufreq/policy{0,4,6}; do
  echo "$p $(cat "$p/scaling_governor") $(cat "$p/scaling_max_freq")"
done
```

To disable the persistent cap and restore the original limits immediately:

```bash
sudo systemctl disable --now rk3588-cpu-cap.service
echo 1800000 | sudo tee /sys/devices/system/cpu/cpufreq/policy0/scaling_max_freq
echo 2256000 | sudo tee /sys/devices/system/cpu/cpufreq/policy4/scaling_max_freq
echo 2256000 | sudo tee /sys/devices/system/cpu/cpufreq/policy6/scaling_max_freq
```

For a one-time experiment the percentage is overridable without editing the
installed script:

```bash
sudo env RK3588_CPU_LIMIT_PERCENT=80 \
  /usr/local/sbin/rk3588-cpu-cap
```

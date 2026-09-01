# Asterinas NixOS guest 启动慢排查与修复记录

本文记录 2026-09-01 对 Asterinas NixOS guest 启动从 20 多秒退化到 100~160 秒问题的排查、验证和修复过程。本文是 debug 文档，重点保留诊断思路、证据链和后续复查入口；若本文与当前代码或实际命令结果不一致，以当前代码和命令结果为准。

## 1. 问题现象

### 1.1 用户可见现象

在 `dm` 分支做 Device Mapper 系统测试时，NixOS guest ready 时间明显变慢：

```text
正常预期：重启外层 VM 后，guest 启动约 20 多秒。
异常状态：外层 VM 用久后，guest 启动约 100~160 秒。
影响：LVM2/DM 系统测试每个 guest 生命周期默认 180 秒，容易被 ready 慢耗尽预算。
```

代表性失败现象：

```text
--linear-lvm2-cross-segment
first_guest_ready_after=158s
随后因 first_guest_lifecycle_timeout=180s 失败
```

使用更长超时后，语义本身能通过：

```text
GUEST_QEMU_TIMEOUT=300 --linear-lvm2-cross-segment
first_guest_ready_after=100s
second_guest_ready_after=148s
HOST_PASS_DM_SYSTEM_TESTS --linear-lvm2-cross-segment
```

这说明当时主要问题不是 DM/LVM2 语义错误，而是 guest 启动/时间推进异常。

### 1.2 排查边界

本轮排查遵守以下边界：

```text
不默认重启 myAsterinas 容器作为解法。
尽量不重启外层 VMware VM。
不擅自改变 KVM/RELEASE/启动协议等基础启动设置作为最终修复。
QEMU、ktest、NixOS 系统测试串行执行，避免镜像锁和残留进程互相干扰。
```

容器重启不是根因。用户明确指出问题来自外层 VM 用久后 guest 启动变慢，重启外层 VM 后恢复；因此排查重点转向 QEMU/KVM、Asterinas timer、systemd/getty 和 host/guest 时间差。

## 2. 初始排查与排除项

### 2.1 资源和 cgroup 排查

已排除常规资源瓶颈：

```text
host CPU idle 高，steal=0。
I/O wait 低。
容器 cgroup CPU quota 为 -1。
cpu.stat nr_throttled=0。
cpuset 覆盖 0-11。
```

结论：不是容器 CPU 限流，也不是 host 明显资源打满。

### 2.2 KVM 状态排查

容器内可见：

```text
/dev/kvm 可访问。
CPU flags 有 vmx。
kvm_intel nested=Y。
ept=Y。
```

QEMU 参数实际包含 KVM：

```text
-accel kvm
```

关闭 KVM 的对照也同样慢：

```text
ENABLE_KVM=0:
EFI_STUB     8s
STAGE2       14s
SYSTEMD      16s
WELCOME      123s
ROOT_PROMPT  125s
```

结论：不是单纯 `-accel kvm` 是否开启导致。

### 2.3 镜像 page cache 预热排查

用户提出 `cat target/nixos/asterinas.img >/dev/null` 预热缓存假设。实测读取镜像：

```text
第一次 real 1m37.920s
第二次 real 0m43.090s
```

预热后 guest 仍然较慢：

```text
BOOT_AFTER_WARM_RESULT ready_after=126s
```

结论：缓存预热有一定影响，但不是 100~160 秒慢启动主因。

### 2.4 MEM 对照

使用 `MEM=2G` 对照：

```text
EFI_STUB 7s
STAGE2 11s
SYSTEMD 11s
WELCOME/LOGIN/ROOT_PROMPT 154s
```

结论：不是 `MEM=8G` 导致。

## 3. 关键突破：host wall clock 与 guest uptime 不一致

### 3.1 诊断方法

构造最小启动诊断：

1. host 侧记录 `EFI stub`、`starting systemd`、`Welcome to`、`root@asterinas` 出现的墙钟时间。
2. guest 进入 root prompt 后执行：
   - `date +%s`
   - `cat /proc/uptime`
   - `systemctl list-jobs`
   - `systemctl --failed`
   - hvc0/getty 相关 `systemctl status/show`

一次代表性结果：

```text
HOST_MARKER EFI_STUB elapsed=6s
HOST_MARKER SYSTEMD elapsed=9s
HOST_MARKER WELCOME elapsed=143s
HOST_MARKER ROOT_PROMPT elapsed=143s
GUEST_UPTIME 10.58 8.60
```

含义：host 已经过了 143 秒，但 guest 自己的 `/proc/uptime` 只有约 10.6 秒。

### 3.2 进一步 sleep 比例验证

关闭 guest tty echo 后，在 guest 内执行：

```sh
for i in 0 1 2 3; do
    printf "OUT_TICK_$i "
    date +%s
    printf " "
    cat /proc/uptime
    sleep 1
done
```

KVM 下结果：

```text
HOST_SEEN_OUT_TICK_0 elapsed=117s
HOST_SEEN_OUT_TICK_1 elapsed=130s
HOST_SEEN_OUT_TICK_2 elapsed=142s
HOST_SEEN_OUT_TICK_3 elapsed=157s
```

TCG 下结果：

```text
HOST_SEEN_OUT_TICK_0 elapsed=146s
HOST_SEEN_OUT_TICK_1 elapsed=161s
HOST_SEEN_OUT_TICK_2 elapsed=175s
HOST_SEEN_OUT_TICK_3 elapsed=190s
```

结论：guest 内 `sleep 1` 实际需要 host 约 12~15 秒。问题不是单纯 console 输出慢，而是 guest timer/timekeeping 本身慢。

## 4. Asterinas timekeeping 代码路径

### 4.1 `/proc/uptime` 来源

`/proc/uptime` 直接读取 monotonic time：

```rust
aster_time::read_monotonic_time()
```

位置：

```text
kernel/core/src/fs/fs_impls/procfs/uptime.rs
```

### 4.2 monotonic clocksource 来源

Asterinas 当前默认 time clocksource 是 TSC：

```rust
ClockSource::new(
    tsc_freq(),
    MAX_DELAY_SECS,
    Arc::new(read_tsc),
)
```

位置：

```text
kernel/core/comps/time/src/tsc.rs
```

### 4.3 TSC 频率来源

TSC 频率初始化路径：

```rust
let tsc_freq = determine_tsc_freq_via_cpuid().unwrap_or_else(determine_tsc_freq_via_pit);
```

位置：

```text
ostd/src/arch/x86/kernel/tsc.rs
```

优先使用 CPUID leaf 0x15；不可用时用 PIT 校准。

### 4.4 APIC timer 也依赖 TSC 频率

APIC periodic/deadline timer 初始化会使用同一个 `tsc_freq()`：

```rust
let tsc_interval = tsc_freq() / TIMER_FREQ;
```

位置：

```text
ostd/src/arch/x86/timer/apic.rs
```

因此 TSC 频率错误会同时影响：

```text
/proc/uptime
clock_gettime/date/sleep 等用户态时间感知
systemd timer 和 unit timeout
APIC timer tick 行为
```

## 5. 关键根因：TSC 频率异常高

### 5.1 外层 Linux/VMware 报告的真实量级

外层 VM dmesg：

```text
vmware: TSC freq read from hypervisor : 2495.998 MHz
tsc: Detected 2495.998 MHz processor
```

真实量级约为：

```text
2.496 GHz
```

### 5.2 Asterinas 异常读数

修复前，在 ktest info 日志中看到：

```text
KVM: INFO: TSC frequency: 24143725390 Hz
TCG: INFO: TSC frequency: 23028628220 Hz
后续 PIT fallback 也曾得到 33345655870 / 34030706940 Hz
```

也就是 Asterinas 认为 TSC 频率是：

```text
23~34 GHz
```

比外层 VM 报告的约 2.5GHz 高接近一个数量级。

### 5.3 为什么会导致 guest 变慢

TSC clocksource 的基本换算是：

```text
elapsed_ns = tsc_delta / tsc_freq
```

如果 `tsc_freq` 被错误放大约 10 倍，那么同样的真实 TSC delta 被换算成的 guest 时间就只有约 1/10。

表现就是：

```text
host 经过 120 秒
Asterinas guest 只认为经过十几秒
guest sleep 1 秒需要 host 等 12~15 秒
systemd 和 shell 登录也随之被放大
```

### 5.4 CPUID 与 PIT 都不可靠

尝试过多个来源：

```text
CPUID leaf 0x15：可返回异常频率。
PIT fallback：在当前 QEMU/VMware 状态下也会校准出 30GHz 级别异常频率。
CPUID leaf 0x16：当前 QEMU CPU 暴露下不可用。
hypervisor timing leaf 0x40000010：当前 QEMU/KVM 暴露下不可用。
```

因此只“从 CPUID 改到 PIT”不够，必须对 QEMU/虚拟化环境中的频率来源做可信度过滤。

## 6. TSC 修复方案

### 6.1 目标

原来行为：

```text
QEMU/VMware 环境下可能采用 23~34GHz 的异常 TSC 频率。
guest sleep 1 秒约等于 host 12~15 秒。
NixOS guest ready 100~160 秒。
```

目标行为：

```text
QEMU/VMware 环境下过滤明显不可信的 TSC 频率。
使用可用的虚拟化频率来源；不可用时保守 fallback 到当前环境真实量级 2.5GHz。
guest sleep 1 秒接近 host 1 秒。
```

### 6.2 代码改动

修改文件：

```text
ostd/src/arch/x86/cpu/cpuid.rs
ostd/src/arch/x86/kernel/tsc.rs
```

新增 CPUID 查询：

```text
leaf 0x16: ProcessorFreq
leaf 0x40000010: HypervisorTiming
```

QEMU 环境下 TSC 频率选择顺序：

```text
1. hypervisor-reported TSC frequency
2. processor base frequency
3. CPUID leaf 0x15 TSC frequency
4. PIT fallback
5. QEMU 保守 fallback：2_500_000_000 Hz
```

同时加入明显异常值过滤：

```text
MAX_PLAUSIBLE_TSC_FREQ = 10_000_000_000 Hz
```

即 QEMU 环境下大于 10GHz 的值不直接采用。

### 6.3 验证

修复后 ktest 日志：

```text
INFO: TSC frequency: 2500000000 Hz
INFO: timer: Enable APIC periodic mode
test result: ok. 1 passed; 0 failed; 183 filtered out.
```

重建 NixOS 后，guest sleep 恢复正常：

```text
OUT_TICK_0 1788167306
OUT_TICK_1 1788167307
OUT_TICK_2 1788167308
OUT_TICK_3 1788167309
```

对应 host 侧：

```text
HOST_SEEN_OUT_TICK_0 elapsed=83s
HOST_SEEN_OUT_TICK_1 elapsed=84s
HOST_SEEN_OUT_TICK_2 elapsed=85s
HOST_SEEN_OUT_TICK_3 elapsed=86s
```

结论：timer 失真问题修复。

## 7. 第二层问题：getty 配置拖慢 prompt

### 7.1 现象

TSC 修复后，guest sleep 已恢复 1:1，但最小 `make run_nixos` 仍出现过：

```text
ROOT_PROMPT 81s
ROOT_PROMPT 59s
ROOT_PROMPT 41s
```

继续诊断发现 systemd job 队列中有：

```text
dev-hvc0.device          start running
serial-getty@hvc0.service start waiting
getty.target              start waiting
multi-user.target         start waiting
```

同时实际可用的是：

```text
getty@hvc0.service active/running
```

即 hvc0 上同时存在普通 getty 与 serial-getty 两条路径。

### 7.2 配置来源

`tools/qemu_args.sh` 默认使用：

```text
CONSOLE=hvc0
-device virtconsole,chardev=mux
```

NixOS kernel cmdline 使用：

```text
console=hvc0
```

`distro/etc_nixos/modules/systemd.nix` 原本会把多个 autovt 加入 `getty.target`：

```text
autovt@hvc0.service
autovt@tty1.service
autovt@tty2.service
...
autovt@tty6.service
```

系统还会派生/启用 `serial-getty@hvc0.service`，它等待 `dev-hvc0.device`，而 `/dev/hvc0` 虽存在，但 systemd device unit 没有正常 active。

### 7.3 修复一：禁用重复 serial-getty

修改文件：

```text
distro/etc_nixos/modules/systemd.nix
```

加入：

```nix
systemd.services."serial-getty@hvc0".enable = false;
```

验证后：

```text
serial-getty@hvc0.service masked inactive dead
systemd job 队列为空
```

启动改善到：

```text
ROOT_PROMPT 25~29s
```

### 7.4 修复二：只保留 hvc0 getty

继续发现仍会拉起 `tty1~tty6` 多个 root autologin shell。测试和 `make run_nixos` 实际只需要 hvc0，因此收窄 getty target：

```nix
systemd.targets.getty.wants = lib.mkForce [ "autovt@hvc0.service" ];
```

效果：

```text
最小 make run_nixos:
EFI_STUB     6s
SYSTEMD      9s
WELCOME      18~19s
ROOT_PROMPT  18~19s
```

注意：诊断输出里仍能看到 `getty@tty1.service active` 的一次样本，说明 NixOS 可能还有其他默认路径拉起 tty1；但从 wall clock 看，`mkForce` 已把主要启动成本压下去。后续若要继续极限优化，可再单独追 tty1 的来源。

## 8. 最终验证结果

### 8.1 最小启动

修复后最小 `make run_nixos`：

```text
HOST_MARKER EFI_STUB elapsed=6s
HOST_MARKER SYSTEMD elapsed=9s
HOST_MARKER WELCOME elapsed=18~19s
HOST_ROOT_PROMPT elapsed=18~19s
```

### 8.2 linear cross-segment 系统测试

重启外层 VM 后，TSC/getty 修复前后都跑过代表性脚本；最终优化后结果：

```text
GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --linear-lvm2-cross-segment

first_guest_ready_after=25s
second_guest_ready_after=18s
TEST_PASS_DM_LINEAR_LVM2_CROSS_SEGMENT_FIRST
TEST_PASS_DM_LINEAR_LVM2_CROSS_SEGMENT_SECOND
HOST_PASS_DM_LINEAR_LVM2_CROSS_SEGMENT
HOST_PASS_DM_SYSTEM_TESTS --linear-lvm2-cross-segment
```

### 8.3 striped cross-segment 系统测试

最终优化后结果：

```text
GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --striped-lvm2-cross-segment

first_guest_ready_after=25s
second_guest_ready_after=25s
TEST_PASS_DM_STRIPED_LVM2_CROSS_SEGMENT_FIRST
TEST_PASS_DM_STRIPED_LVM2_CROSS_SEGMENT_SECOND
HOST_PASS_DM_STRIPED_LVM2_CROSS_SEGMENT
HOST_PASS_DM_SYSTEM_TESTS --striped-lvm2-cross-segment
```

## 9. 排查结论

本次启动慢是两个问题叠加：

1. **主要根因：Asterinas 在 QEMU/VMware 环境中采用了异常 TSC 频率。**
   - Asterinas 读到 23~34GHz。
   - 外层 VMware/Linux 报告真实量级约 2.5GHz。
   - 导致 guest 时间推进比 host 慢约 10 倍。

2. **第二层拖慢：NixOS getty 配置同时拉起重复/无用登录路径。**
   - `serial-getty@hvc0` 等待 `dev-hvc0.device`。
   - 多个 tty getty/root autologin 增加启动成本。
   - 禁用重复 serial-getty 并收窄到 hvc0 后，最小启动压到 18~19 秒。

最终效果：

```text
异常前：guest ready 100~160s，默认 180s 生命周期容易超时。
TSC 修复后：guest sleep 1s 恢复为 host 约 1s。
getty 优化后：最小 make run_nixos 18~19s。
代表性 DM/LVM2 系统测试 ready 18~25s，并全部通过。
```

## 10. 后续注意事项

### 10.1 当前 TSC fallback 的边界

当前 `2.5GHz` 是面向当前 QEMU/VMware 环境的保守兜底，不是完整通用时钟框架。更长期的工程方向应考虑：

```text
引入更可靠的虚拟化 clocksource。
支持 ACPI PM timer / HPET 作为校准或 clocksource。
完善 QEMU/KVM hypervisor leaf 识别。
避免在虚拟化环境中信任明显异常的 PIT 校准结果。
```

### 10.2 当前 getty 优化的边界

只保留 hvc0 getty 适合当前自动化测试和 `make run_nixos` 控制台。如果后续需要图形桌面、tty 登录或多 console 场景，应重新评估：

```text
是否需要 tty1。
是否需要 serial-getty。
是否应按 console 参数动态决定 getty 列表。
```

### 10.3 复查命令

TSC 频率复查：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && timeout -k 10s 180s make ktest CARGO_OSDK_TEST_ARGS="validates_ioctl_encoding_and_alignment --kcmd-args=loglevel=info --kcmd-args=earlycon --kcmd-args=console=ttyS0 --boot-method=grub-rescue-iso --grub-boot-protocol=multiboot2" 2>&1 | grep -aE "TSC frequency|Enable APIC|APIC timer|test result"'
```

最小启动复查：

```text
启动 make run_nixos，记录 EFI_STUB / SYSTEMD / WELCOME / ROOT_PROMPT 时间，并在 guest 中验证 sleep 1 秒约等于 host 1 秒。
```

代表性系统测试复查：

```bash
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --linear-lvm2-cross-segment'
docker exec myAsterinas bash -lc 'cd /root/asterinas && GUEST_QEMU_TIMEOUT=180 myshell/run_dm_system_tests.sh --striped-lvm2-cross-segment'
```

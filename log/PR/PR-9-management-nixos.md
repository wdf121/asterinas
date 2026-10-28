# PR9：NixOS dmsetup 测试

## 1. 状态与 Git 标识

- 提交：`24e7ee2e8`；直接父提交：`1e0415869`；本地 `pr-dm` HEAD。
- 未建立独立正式分支，未推送本提交。
- 范围：5 文件，+296；Cargo.lock、suite manifest、Rust 入口、cases.sh 和镜像配置。
- 镜像安装脚本为 `/etc/dm-management-cases.sh`，不经串口分段上传。

## 2. 目标与非目标

使用真实 dmsetup/libdevmapper 验证无表管理工作流；不要求 backing 或 target，不以成功结果宣称完整设备管理生态或数据面可用。

## 3. 设计与改动范围

新增官方 NixOS suite：Rust 入口注册三个场景，镜像配置安装 lvm2 和 cases.sh。
脚本使用唯一名称、命令超时和创建前登记的清理流程，仅在清理成功后输出通过标记。

### udev 语义边界

脚本不添加 --noudevsync，不关闭 udev rules，也不以 raw ioctl 替代 dmsetup。
查阅的 LVM2 2.03.39 源码中，create --notable 不设置等待事件的 cookie；remove 路径在内核未报告 DM_UEVENT_GENERATED_FLAG 时由库内完成 cookie。
容器的 nixpkgs 选择及本地 dmsetup store 路径均为 2.03.39，但 Nix 源码归档本地缺失，未完成与所查源码的逐字节比较。
三个场景成功不证明完整 uevent、sysfs 或 udev 自动化已支持。
不把旧 LVM 测试中的 udev_rules=0、udev_sync=0 泛化为本套件的必要条件。


## 4. 历史与远端事件

提交已在本地 pr-dm 制备链保存，尚未建立或推送对应正式分支。

## 5. 验证证据与边界

助手完成格式、差异和 Shell 语法检查。
一次 make check 在 aster-core 阶段以 137 退出，未证明具体原因；用户随后明确确认 make check 通过。
用户构建镜像并在 guest 手动运行全部三个场景，确认全部成功后授权提交。
不将手动场景通过写为助手执行了自动 runner，也不虚构运行时长或检查计数。

## 6. 后续动作

发布前核对上游基线及前置依赖；实际运行证据与复现命令分开记录。
文档更新不代表创建、提交或推送上游页面。

## 7. Maintainer description draft

以下英文标题和正文为维护者阅读的草稿，尚未发布。

### Title

Add NixOS dmsetup tests for Device Mapper management

### Summary

This change adds a NixOS test suite that exercises tableless Device Mapper management through real `dmsetup` and libdevmapper calls.
The image includes `lvm2` and installs the guest cases as `/etc/dm-management-cases.sh`; the standard NixOS test framework launches the VM and checks command completion and case markers.

### Design

The suite covers three workflows:

- Driver/library version reporting and the registered Device Mapper block major.
- Tableless creation, lookup by name, UUID, and device number, inactive-state queries, listing, removal, and creation without a UUID.
- Name, UUID, and minor conflicts, preservation of the original identity after failed creation, and UUID/minor reuse after removal.

Cases use unique names, register cleanup before creation, bound dmsetup calls with timeouts, and reject timeouts or signal termination as substitutes for expected command failures.
Success markers are emitted only after successful cleanup.

### Scope

The suite does not disable udev synchronization or replace dmsetup calls with raw ioctls.
It requires no backing disks or mapping targets and does not use device-node existence as proof of kernel runtime publication.
Passing these cases does not establish complete uevent/udev support or block-I/O functionality.

### Testing

Run from the repository root in the development container, on the corresponding source revision:

```bash
make format
make check
make kernel
make nixos NIXOS_TEST_SUITE=device-mapper-management
make run_nixos NIXOS_TEST_SUITE=device-mapper-management
```

### Review focus

Real dmsetup behavior, bounded failure handling, and cleanup before success markers.

🤖 Generated with [Claude Code](https://claude.com/claude-code)

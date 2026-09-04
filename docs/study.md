# Device Mapper 学习记录

## DM 项目分层总图

这张图先按层理解整个项目：每一层只放核心结构体或关键函数，红色节点是跨层入口。

```mermaid
flowchart TD
    subgraph U["用户空间（Userspace）"]
        U1["dmsetup"]
        U2["LVM2"]
        U3["libdevmapper"]
        U4["Applications / mkfs"]
    end

    subgraph K["Asterinas 内核态（Kernel）"]
        CDEV["/dev/mapper/control<br/>DmControlDevice / DmControlFile"]
        MDEV["/dev/dm-N 与 mapper 别名<br/>DmDevice 块设备节点"]

        subgraph DM["DM 核心引擎（comps/device-mapper）"]
            subgraph CP["第一层：Device Mapper 控制面框架（低频）"]
                CP1["DmControlFile::ioctl<br/>decode_command / handle_command"]
                CP2["DmManager<br/>name / uuid / minor 索引"]
                CP3["DmDeviceState<br/>active / inactive table"]
                CP4["Table load / clear / status<br/>suspend / resume / wait"]
            end

            subgraph DP["第二层：Device Mapper 数据面核心（高频 I/O）"]
                DP1["DmDevice::enqueue<br/>只接受 active table"]
                DP2["DmTable::enqueue<br/>按 logical sector 查 target"]
                DP3["Target dispatch<br/>map_io_range"]
                DP4["BIO remapping / split<br/>remap_sid_start / split"]
                DP5["Flush fan-out<br/>FlushCompletion 聚合"]
            end

            subgraph TG["第三层：Target 映射策略插件"]
                TG1["linear<br/>单后端 + sector offset"]
                TG2["striped<br/>多后端 stripe / chunk"]
                TG3["zero<br/>读零 / 写成功"]
                TG4["error<br/>返回 I/O error"]
            end
        end

        subgraph BLK["Asterinas 块设备框架（comps/block + device registry）"]
            B1["BlockDevice trait"]
            B2["BlockDeviceLease"]
            B3["Bio / SubmittedBio / BioSegment"]
            B4["BioRequestSingleQueue / BioRequest"]
            B5["Block registry<br/>lookup / register / unregister"]
        end

        subgraph BACK["真实后端块设备"]
            D1["virtio-blk"]
            D2["NVMe"]
            D3["PartitionNode"]
        end
    end

    U1 -->|"ioctl"| U3
    U2 -->|"ioctl"| U3
    U3 -->|"Linux DM ABI"| CDEV
    U4 -->|"read / write / mount"| MDEV

    CDEV -->|"ioctl 分发"| CP1
    CP1 --> CP2
    CP2 --> CP3
    CP3 --> CP4
    CP4 -->|"resume 后注册"| MDEV

    MDEV -->|"BIO 提交"| DP1
    DP1 --> DP2
    DP2 --> DP3
    DP3 --> DP4
    DP3 --> DP5
    DP3 --> TG1
    DP3 --> TG2
    DP3 --> TG3
    DP3 --> TG4

    TG1 -->|"Remap"| B2
    TG2 -->|"Remap / split"| B2
    TG3 -->|"Zero"| B3
    TG4 -->|"Error"| B3
    DP4 --> B3
    DP5 --> B3

    B2 --> B1
    B3 --> B1
    B1 --> B4
    B1 --> B5
    B4 --> D1
    B4 --> D2
    B5 --> D3

    classDef userspace fill:#14351f,stroke:#4ade80,stroke-width:2px,color:#f8fafc;
    classDef entry fill:#12385c,stroke:#60a5fa,stroke-width:2px,color:#f8fafc;
    classDef control fill:#5b2a06,stroke:#fb923c,stroke-width:2px,color:#fff7ed;
    classDef dataplane fill:#4a2504,stroke:#f97316,stroke-width:2px,color:#fff7ed;
    classDef target fill:#713f12,stroke:#facc15,stroke-width:2px,color:#fffbeb;
    classDef block fill:#3b173f,stroke:#e879f9,stroke-width:2px,color:#fdf4ff;
    classDef backend fill:#1f2937,stroke:#94a3b8,stroke-width:2px,color:#f8fafc;
    classDef critical fill:#7f1d1d,stroke:#f87171,stroke-width:3px,color:#fff1f2;

    class U1,U2,U3,U4 userspace;
    class CDEV,MDEV entry;
    class CP1,CP2,CP3,CP4 control;
    class DP1,DP2,DP3,DP4,DP5 dataplane;
    class TG1,TG2,TG3,TG4 target;
    class B1,B2,B3,B4,B5 block;
    class D1,D2,D3 backend;
    class CDEV,MDEV,CP1,DP1,DP2,DP3 critical;
```

读这张图时先抓三条线：

```text
控制面：dmsetup / LVM2 -> libdevmapper -> DmControlFile::ioctl -> DmManager -> DmDevice -> DmTable
数据面：ext2 / page cache -> DmDevice -> DmTable -> DmTarget -> TargetIoAction -> Bio -> backing BlockDevice
生命周期：DmTarget -> BlockDeviceLease -> BlockDevice -> block registry
```

## `/dev/mapper/control` 注册主线

这条主线解释 `/dev/mapper/control` 如何从内核启动期注册到用户态可见。重点不是 DM table、target 或 BIO 数据面，而是 DM control 字符设备如何挂到 Asterinas 的 misc/char/devtmpfs 链路下。

### 1. first kthread 阶段

入口在 `misc::init_in_first_kthread()`。

```text
misc::init_in_first_kthread()
  -> acquire_major(MajorId::new(10))
     准备 misc 字符设备 major 10

  -> device_mapper::init_in_first_kthread()
     -> 先初始化 DM_MANAGER
        不是为了注册 control 的硬依赖，
        而是保证 control 暴露给用户态前，后端 manager 已就绪

     -> DmControlDevice::new()
        创建 DM control 字符设备对象
        绑定 DeviceId = misc major 10 + DM_CONTROL_MINOR

     -> char::register(...)
        注册到 char DEVICE_REGISTRY
        建立 DeviceId -> DmControlDevice 的映射
```

### 2. first process 阶段

`char::register(...)` 只完成设备号到设备对象的注册；真正创建 `/dev/mapper/control` 路径节点发生在 first process 阶段。

```text
char::init_in_first_process(path_resolver)
  -> collect_all()
  -> 找到 DmControlDevice
  -> device.devtmpfs_meta()
     返回 "mapper/control"

  -> add_node(DeviceType::Char, dev_id, meta, path_resolver)
     在 /dev 下创建 mapper/control 节点
```

### 3. 阶段结论

最终效果是：

```text
/dev/mapper/control 出现在用户态
用户态 dmsetup 可以 open 它
后续 ioctl 会进入 DmControlFile::ioctl()
再分发到 DM 控制面逻辑
```

理解这条链路时要区分：

- `misc major 10`：给 `/dev/mapper/control` 这个 misc 字符设备使用。
- `DM_MANAGER`：control ioctl 后端依赖的全局管理器，后续管理 mapper block devices。
- `DmControlDevice`：暴露给用户态的 control 字符设备对象。
- `char::register(...)`：建立 `DeviceId -> DmControlDevice` 的内核注册表映射。
- `devtmpfs_meta()`：由 `DmControlDevice` 自己声明用户态路径 `mapper/control`。
- `add_node(...)`：根据 metadata 在 `/dev` 下创建真实路径节点。

## `dmsetup version` ioctl 主线

```text
dmsetup version
  -> libdevmapper
       准备 raw ioctl number = DM_VERSION
       准备 dm_ioctl buffer，其中 version 字段带用户态版本

  -> open("/dev/mapper/control")
  -> DmControlDevice::open()
  -> DmControlFile

  -> ioctl(fd, DM_VERSION, dm_ioctl buffer pointer)
  -> sys_ioctl 根据 fd 找到 DmControlFile
  -> DmControlFile::ioctl()

       raw ioctl number ---------> decode_command() = DM_VERSION_CMD
                                      |
                                      v
       dm_ioctl buffer pointer ---> 读取并校验 dm_ioctl buffer
                                      |
                                      v
                              validate_client_version(buffer)
                              write_version(buffer)
                              handle_command(DM_VERSION_CMD, buffer)
                                      |
                                      v
                              写回 dm_ioctl buffer

  -> dmsetup 打印 Driver version
```

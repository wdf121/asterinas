# Device Mapper 整体架构图草稿

本文只作为技术文档配图草稿，正式位置和正文说明待确认后再合入 [device-mapper-technical-maintenance.md](device-mapper-technical-maintenance.md)。

## 整体取舍建议

- 建议正式文档保留图 1：它是唯一从用户态到 backing block device 的总览图，也是区分 Device Mapper 专属改动与 Asterinas 内核框架改动的入口图。
- 建议图 2 和图 3 二选一或合并引用：如果正文讲控制面细节，就保留两张；如果正文篇幅有限，可合并成一节，只保留 LVM2 图，dmsetup 命令用表格补充。
- 建议图 4 和图 5 同时保留：图 4 讲 Read/Write 主路径，图 5 讲跨 segment 的 BIO 切分细节，层级不同，不算重复。
- 建议图 6 和图 7 作为 LV 生命周期小节的两张图：图 6 讲停用、删除映射和重建恢复，图 7 讲扩容、缩容和 table 切换；如果后续需要压缩，可把图 6 放正文，图 7 放测试说明或附录。
- Flush 数据面，以及 status/deps 与 backing lease 的关系已经补成图 8 和图 9；正式合入时应删除本节取舍建议，只保留正文引用、图和图注。

## 颜色约定

- 蓝色 / 青色：用户态入口、LVM2 元数据或用户态操作。
- 紫色：系统调用边界、BIO 切分或缩容路径。
- 橙色 / 黄色：Device Mapper ioctl、DM table、target 和内核对象。
- 绿色：Asterinas 框架支撑或成功结果。
- 灰色：底层块设备或最终作用。
- 红色：错误路径、风险或失败结果。

## 图 1：从用户态到块设备的完整路线图

```mermaid
flowchart TD
    subgraph U[一、用户态]
        direction TB
        U1["LVM2 / dmsetup<br/>管理 PV / VG / LV<br/>组织 DM table"]
        U2["文件系统 / 应用 I/O<br/>mkfs / mount<br/>read / write"]
    end

    subgraph S[二、系统调用与设备节点]
        direction TB
        S1["控制入口<br/>/dev/mapper/control"]
        S2["映射入口<br/>/dev/mapper/name<br/>/dev/dm-N"]
    end

    subgraph K[三、Asterinas 内核框架改动]
        direction TB
        K1["设备发现与注册<br/>devtmpfs<br/>major:minor / procfs"]
        K2["VFS 与 BIO 框架<br/>mount / submit / complete"]
        K3["backing 生命周期<br/>BlockDeviceLease"]
    end

    subgraph D[四、Device Mapper 专属改动]
        direction TB
        D1["ioctl 控制面<br/>create / load / resume"]
        D2["device / table 生命周期<br/>inactive → active"]
        D3["table 解析与构建<br/>parser / target spec"]
        D4["status / deps 查询<br/>table / deps / info"]
        D5["target 映射<br/>linear / striped"]
        D6["BIO 数据面<br/>split / remap / flush"]
    end

    subgraph B[五、底层块设备]
        direction TB
        B1["backing block device<br/>LVM PV 所在设备"]
        B2["virtio-blk 与驱动队列<br/>read / write / flush"]
    end

    U1 -->|"控制面"| S1
    U2 -->|"数据面"| S2
    S1 --> D1
    S2 --> K2
    K1 --> S1
    K1 --> S2
    K2 --> D5
    D1 --> D2
    D2 --> D3
    D3 --> D4
    D3 --> D5
    D5 --> D6
    D6 --> B1
    D3 --> K3
    K3 --> B1
    B1 --> B2

    classDef user fill:#e8f3ff,stroke:#2563eb,stroke-width:1px,color:#0f172a;
    classDef syscall fill:#eef2ff,stroke:#4f46e5,stroke-width:1px,color:#0f172a;
    classDef kernel fill:#ecfdf5,stroke:#059669,stroke-width:1px,color:#0f172a;
    classDef dm fill:#fff7ed,stroke:#ea580c,stroke-width:2px,color:#0f172a;
    classDef block fill:#f8fafc,stroke:#475569,stroke-width:1px,color:#0f172a;

    class U1,U2 user;
    class S1,S2 syscall;
    class K1,K2,K3 kernel;
    class D1,D2,D3,D4,D5,D6 dm;
    class B1,B2 block;
```

## 图 2：dmsetup 控制面命令、ioctl 与结果

```mermaid
flowchart TD
    subgraph C1[一、dmsetup 用户态命令]
        direction TB
        A1["发现能力<br/>version / targets"]
        A2["创建设备<br/>create"]
        A3["查询状态<br/>table / status / deps<br/>info / ls"]
        A4["生命周期操作<br/>rename / suspend<br/>resume / wait"]
        A5["删除设备<br/>remove / remove_all"]
    end

    subgraph C2[二、对应 DM ioctl]
        direction TB
        B1["能力查询<br/>DM_VERSION<br/>DM_LIST_VERSIONS"]
        B2["设备管理<br/>DM_DEV_CREATE<br/>DM_DEV_RENAME / REMOVE"]
        B3["table 切换<br/>DM_TABLE_LOAD<br/>DM_DEV_SUSPEND"]
        B4["状态查询<br/>DM_TABLE_STATUS<br/>DM_TABLE_DEPS"]
        B5["事件等待<br/>DM_DEV_WAIT"]
    end

    subgraph C3[三、最终结果]
        direction TB
        C1R["target 可见<br/>linear / striped"]
        C2R["设备出现或删除<br/>mapper name / dm-N"]
        C3R["table 生效<br/>inactive → active"]
        C4R["状态可查询<br/>status / deps / info"]
        C5R["状态变化<br/>suspended / removed"]
    end

    A1 --> B1 --> C1R
    A2 --> B2 --> C2R
    A2 --> B3 --> C3R
    A3 --> B4 --> C4R
    A4 --> B2
    A4 --> B3
    A4 --> B5
    A5 --> B2 --> C5R

    classDef user fill:#e8f3ff,stroke:#2563eb,stroke-width:1px,color:#0f172a;
    classDef ioctl fill:#fff7ed,stroke:#ea580c,stroke-width:2px,color:#0f172a;
    classDef result fill:#ecfdf5,stroke:#059669,stroke-width:1px,color:#0f172a;

    class A1,A2,A3,A4,A5 user;
    class B1,B2,B3,B4,B5 ioctl;
    class C1R,C2R,C3R,C4R,C5R result;
```

## 图 3：LVM2 控制面命令、ioctl 与结果

```mermaid
flowchart TD
    subgraph L1[一、LVM2 用户态命令]
        direction TB
        L1A["PV / VG 元数据<br/>pvcreate / vgcreate<br/>vgextend"]
        L1B["LV 创建与调整<br/>lvcreate / lvextend<br/>lvreduce"]
        L1C["LV 激活与停用<br/>vgchange -ay<br/>vgchange -an"]
        L1D["元数据查询<br/>pvs / vgs / lvs<br/>lvs --segments"]
        L1E["重启后扫描<br/>pvscan<br/>vgscan --mknodes"]
    end

    subgraph L2[二、libdevmapper 与 DM ioctl]
        direction TB
        L2A["组织 table<br/>linear / striped segment<br/>mixed table"]
        L2B["创建设备<br/>DM_DEV_CREATE"]
        L2C["装载 table<br/>DM_TABLE_LOAD"]
        L2D["激活或停用<br/>DM_DEV_SUSPEND"]
        L2E["查询 table<br/>DM_TABLE_STATUS<br/>DM_TABLE_DEPS"]
    end

    subgraph L3[三、最终结果]
        direction TB
        L3A["PV / VG 元数据写入"]
        L3B["LV 对应 mapper 设备"]
        L3C["active table 更新"]
        L3D["segment 布局确定<br/>linear / striped<br/>mixed table 组合"]
        L3E["重启后恢复 VG/LV"]
    end

    L1A --> L3A
    L1B --> L2A
    L2A --> L2B --> L3B
    L2A --> L2C --> L3D
    L1C --> L2D --> L3C
    L1D --> L2E --> L3D
    L1E --> L1C --> L3E

    classDef lvm fill:#ecfeff,stroke:#0891b2,stroke-width:1px,color:#0f172a;
    classDef ioctl fill:#fff7ed,stroke:#ea580c,stroke-width:2px,color:#0f172a;
    classDef result fill:#ecfdf5,stroke:#059669,stroke-width:1px,color:#0f172a;

    class L1A,L1B,L1C,L1D,L1E lvm;
    class L2A,L2B,L2C,L2D,L2E ioctl;
    class L3A,L3B,L3C,L3D,L3E result;
```

图 2 和图 3 说明：

- `dmsetup` 是更直接的 DM 控制面：脚本里用到 `dmsetup version`、`targets`、`create`、`table`、`status`、`deps`、`info`、`ls`、`rename`、`suspend`、`resume`、`wait`、`remove`、`remove_all`。
- `LVM2` 是更高层的存储管理控制面：脚本里用到 `pvcreate`、`vgcreate`、`vgextend`、`lvcreate`、`lvextend`、`lvreduce`、`vgchange -ay/-an`、`pvscan`、`vgscan --mknodes`、`pvs`、`vgs`、`lvs`、`lvs --segments`。
- `pvcreate`、`vgcreate`、`vgextend`、`pvs`、`vgs`、`lvs` 主要处理或查询 LVM 元数据；真正创建、加载、激活、停用 DM 映射设备时，LVM2 通过 libdevmapper 进入 `/dev/mapper/control` 并触发 DM ioctl。
- 最终结果包括 target 能力可见、mapper 设备出现、table 从 inactive 切到 active、linear / striped segment 布局确定、mixed table 由 linear segment 和 striped segment 组合而成、status/deps/info 可返回，以及重启后 VG/LV 可扫描恢复。

## 图 4：Read / Write BIO 数据面转发路径

```mermaid
flowchart TD
    subgraph A[一、I/O 来源]
        direction TB
        A1["应用或文件系统<br/>read / write / fsync"]
        A2["mapper 块设备<br/>/dev/mapper/name<br/>/dev/dm-N"]
    end

    subgraph B[二、Asterinas 块 I/O 框架]
        direction TB
        B1["构造 BIO<br/>op / sector / len / buffer"]
        B2["提交到 DM 设备<br/>submit_bio"]
        B3["完成回调<br/>Ok 或 IoError"]
    end

    subgraph C[三、Device Mapper 数据面]
        direction TB
        C1["检查设备状态<br/>active table / readonly"]
        C2["按 table 查找范围<br/>定位 segment / target"]
        C3["必要时拆分 BIO<br/>跨 segment / target"]
        C4["linear remap<br/>sector 加 backing_start"]
        C5["striped remap<br/>chunk / row / backing"]
        C6["聚合子 BIO<br/>等待全部完成"]
    end

    subgraph D[四、底层块设备]
        direction TB
        D1["生成子 BIO<br/>backing sector / len"]
        D2["入队 backing 设备<br/>enqueue / submit"]
        D3["驱动完成 I/O<br/>success / error"]
    end

    A1 --> A2
    A2 --> B1 --> B2 --> C1
    C1 -->|"Write 且 readonly"| B3
    C1 -->|"允许 Read/Write"| C2
    C2 --> C3
    C3 -->|"linear target"| C4
    C3 -->|"striped target"| C5
    C4 --> D1
    C5 --> D1
    D1 --> D2 --> D3 --> C6 --> B3
    B3 -->|"唤醒原请求"| A1

    classDef app fill:#e8f3ff,stroke:#2563eb,stroke-width:1px,color:#0f172a;
    classDef framework fill:#ecfdf5,stroke:#059669,stroke-width:1px,color:#0f172a;
    classDef dm fill:#fff7ed,stroke:#ea580c,stroke-width:2px,color:#0f172a;
    classDef block fill:#f8fafc,stroke:#475569,stroke-width:1px,color:#0f172a;

    class A1,A2 app;
    class B1,B2,B3 framework;
    class C1,C2,C3,C4,C5,C6 dm;
    class D1,D2,D3 block;
```

图 4 说明：

- Read 和 Write 的主路径相同：都从 mapper 块设备进入 DM active table，再按逻辑 sector 找到目标 segment。
- 如果一个 BIO 覆盖多个 segment 或 target，DM 数据面会拆成多个子 BIO；每个子 BIO 只落到一个 backing 设备的连续范围。
- `linear` target 的核心是把逻辑 sector 加上 backing 起始偏移；`striped` target 的核心是根据 chunk size 和 stripe row 选择 backing 设备与 backing sector。
- 子 BIO 下发到底层 backing 设备队列后，由 completion 聚合结果：全部成功则原 BIO 成功；任一子 BIO 入队失败或完成为 `IoError`，原 BIO 返回 `IoError`。
- readonly mapper 下 Read 允许，Write 在进入 remap 前被拒绝；Flush 不走本图的 sector remap 路径，后续单独画。

## 图 5：BIO 切分、映射与完成聚合细节

```mermaid
flowchart TD
    subgraph I[一、原始 BIO]
        direction TB
        I1["原 BIO<br/>start + len"]
        I2["查找 table<br/>按逻辑 sector 定位 segment"]
    end

    subgraph S[二、segment 命中情况]
        direction TB
        S1["单段命中<br/>BIO 完全落在一个 segment"]
        S2["多段命中<br/>BIO 跨 segment 边界"]
        S3["越界或空洞<br/>不属于任何 table 范围"]
    end

    subgraph P[三、切分策略]
        direction TB
        P1["无需切分<br/>保留一个子 BIO"]
        P2["按边界切分<br/>每段生成一个子 BIO"]
        P3["立即失败<br/>原 BIO 返回 IoError"]
    end

    subgraph M[四、target 映射]
        direction TB
        M1["按所属 segment 分发<br/>每个子 BIO 独立选择 target"]
        M2["linear segment<br/>backing = start_offset + delta"]
        M3["striped segment<br/>chunk / row / disk 计算"]
        M4["mixed table<br/>linear segment + striped segment"]
    end

    subgraph Q[五、子 BIO 下发]
        direction TB
        Q1["子 BIO A<br/>backing dev + sector + len"]
        Q2["子 BIO B<br/>backing dev + sector + len"]
        Q3["入队到底层设备<br/>enqueue / submit"]
    end

    subgraph C[六、完成聚合]
        direction TB
        C1["等待全部子 BIO 完成"]
        C2["全部成功<br/>原 BIO 返回 Ok"]
        C3["任一失败<br/>原 BIO 返回 IoError"]
    end

    I1 --> I2
    I2 --> S1
    I2 --> S2
    I2 --> S3

    S1 --> P1
    S2 --> P2
    S3 --> P3

    P1 --> M1
    P2 --> M1
    M1 -->|"linear"| M2
    M1 -->|"striped"| M3
    M1 -->|"跨不同 target"| M4
    M4 --> M2
    M4 --> M3

    M2 --> Q1
    M3 --> Q2
    Q1 --> Q3
    Q2 --> Q3
    Q3 --> C1
    C1 --> C2
    C1 --> C3
    P3 --> C3

    classDef input fill:#e8f3ff,stroke:#2563eb,stroke-width:1px,color:#0f172a;
    classDef split fill:#eef2ff,stroke:#4f46e5,stroke-width:1px,color:#0f172a;
    classDef dm fill:#fff7ed,stroke:#ea580c,stroke-width:2px,color:#0f172a;
    classDef block fill:#f8fafc,stroke:#475569,stroke-width:1px,color:#0f172a;
    classDef result fill:#ecfdf5,stroke:#059669,stroke-width:1px,color:#0f172a;
    classDef error fill:#fee2e2,stroke:#dc2626,stroke-width:1px,color:#0f172a;

    class I1,I2 input;
    class S1,S2,P1,P2 split;
    class M1,M2,M3,M4 dm;
    class Q1,Q2,Q3 block;
    class C1,C2 result;
    class S3,P3,C3 error;
```

图 5 说明：

- 单段 segment 场景下，BIO 不需要按 table 边界拆分，只生成一个子 BIO，然后按该 segment 的 target 类型进入 `linear` 或 `striped` 映射逻辑。
- 多段 segments 场景下，BIO 会先按 table segment 边界切分；切分后的每个子 BIO 只覆盖一个 target 的连续逻辑范围，可能是 linear+linear、striped+striped，也可能是 mixed table 中的 linear+striped。
- mixed table 不是一种 target，也不是单个 segment 类型；它表示同一张 table 同时包含 linear segment 和 striped segment，这也是 mixed 测试要覆盖跨 target 读写的原因。
- `linear` 映射只需要计算 backing 起始偏移；`striped` 映射需要根据 chunk size、stripe row 和 stripe index 选择 backing 设备与 backing sector。
- 完成聚合以原 BIO 为单位：所有子 BIO 成功，原 BIO 才成功；任何子 BIO 入队失败或完成失败，原 BIO 返回 `IoError`。

## 图 6：LV 停用、删除映射与重建恢复

```mermaid
flowchart TD
    subgraph A[一、用户态操作]
        direction TB
        A1["停用 LV<br/>vgchange -an"]
        A2["重启后扫描<br/>pvscan / vgscan"]
        A3["重新激活 LV<br/>vgchange -ay"]
    end

    subgraph B[二、LVM 元数据]
        direction TB
        B1["PV / VG / LV 元数据<br/>保留在 backing disk"]
        B2["记录 segment 布局<br/>linear / striped segment"]
        B3["记录 LV 名称与大小"]
    end

    subgraph C[三、DM ioctl 控制面]
        direction TB
        C1["停用映射<br/>DM_DEV_SUSPEND / REMOVE"]
        C2["创建设备<br/>DM_DEV_CREATE"]
        C3["装载 table<br/>DM_TABLE_LOAD"]
        C4["恢复运行<br/>DM_DEV_SUSPEND resume"]
    end

    subgraph D[四、DM 内核对象]
        direction TB
        D1["DmDevice 移除<br/>mapper 节点消失"]
        D2["重新创建 DmDevice<br/>minor / name / uuid"]
        D3["重建 table<br/>重新解析 target 参数"]
        D4["重新持有 deps<br/>获取 backing lease"]
    end

    subgraph E[五、最终结果]
        direction TB
        E1["停用后<br/>LV 不再 active"]
        E2["重建后<br/>/dev/mapper/name 可访问"]
        E3["table 与 deps 恢复<br/>I/O 可继续校验"]
    end

    A1 --> C1 --> D1 --> E1

    A2 --> B1
    B1 --> B2
    B1 --> B3
    B2 --> A3
    B3 --> A3

    A3 --> C2 --> D2 --> C3 --> D3 --> C4 --> D4
    D2 --> E2
    D4 --> E3

    classDef user fill:#e8f3ff,stroke:#2563eb,stroke-width:1px,color:#0f172a;
    classDef meta fill:#ecfeff,stroke:#0891b2,stroke-width:1px,color:#0f172a;
    classDef ioctl fill:#fff7ed,stroke:#ea580c,stroke-width:2px,color:#0f172a;
    classDef dm fill:#fef3c7,stroke:#d97706,stroke-width:1px,color:#0f172a;
    classDef result fill:#ecfdf5,stroke:#059669,stroke-width:1px,color:#0f172a;

    class A1,A2,A3 user;
    class B1,B2,B3 meta;
    class C1,C2,C3,C4 ioctl;
    class D1,D2,D3,D4 dm;
    class E1,E2,E3 result;
```

图 6 说明：

- 当前系统脚本主要验证的是 `vgchange -an` 停用 LV、guest 重启后 `pvscan` / `vgscan --mknodes` 重新发现元数据、再通过 `vgchange -ay` 重建 mapper 设备和 active table。
- 这里的“重建”不是重新创建文件系统数据，也不是重新 `pvcreate/vgcreate/lvcreate`；它依赖 backing disk 上已有的 LVM 元数据恢复 VG/LV 和 segment 布局。
- `vgchange -an` 会让 DM 映射设备停用或消失；重新激活时，LVM2 通过 libdevmapper 按顺序触发 `DM_DEV_CREATE`、`DM_TABLE_LOAD` 和 resume，最终重新得到 `/dev/mapper/name`、active table、status/deps 与 backing lease。
- `dmsetup remove` 也可以直接删除 DM 映射设备，但它不是当前 reboot recovery 脚本的主路径，因此不放在主图中。
- 真正的 `lvremove` 会删除 LVM 元数据里的 LV 定义，属于破坏性删除；当前 reboot recovery 脚本不依赖这种语义，图中重点是“停用映射后基于元数据重建”。

## 图 7：LV 扩容、缩容与 DM table 切换

```mermaid
flowchart TD
    subgraph A[一、扩容路径]
        direction TB
        A1["扩容 LV<br/>lvextend"]
        A2["LVM 元数据变大<br/>追加或扩大 segment"]
        A3["扩文件系统<br/>resize2fs"]
        A4["新增空间可用"]
    end

    subgraph B[二、缩容路径]
        direction TB
        B0["删除扩容区文件<br/>重写 md5"]
        B1["缩文件系统<br/>resize2fs"]
        B2["缩容 LV<br/>lvreduce"]
        B3["LVM 元数据变小<br/>裁剪或删除末段"]
        B4["尾部范围不可访问"]
        B5["错误顺序风险<br/>先缩 LV 会丢数据"]
    end

    subgraph C[三、DM table 切换]
        direction TB
        C1["生成新 table<br/>按最新 segment 布局"]
        C2["装载 inactive table<br/>DM_TABLE_LOAD"]
        C3["切换为 active<br/>DM_DEV_SUSPEND resume"]
    end

    subgraph D[四、DM 内核状态]
        direction TB
        D1["旧 active table<br/>切换前继续存在"]
        D2["新 active table<br/>范围与 deps 更新"]
        D3["backing lease<br/>新增或释放依赖"]
    end

    A1 --> A2 --> C1
    C1 --> C2 --> D1
    C2 --> C3 --> D2 --> D3
    D2 --> A3 --> A4

    B0 --> B1 --> B2 --> B3 --> C1
    D2 --> B4
    B2 -->|"若未先缩文件系统"| B5

    classDef grow fill:#e8f3ff,stroke:#2563eb,stroke-width:1px,color:#0f172a;
    classDef shrink fill:#eef2ff,stroke:#4f46e5,stroke-width:1px,color:#0f172a;
    classDef ioctl fill:#fff7ed,stroke:#ea580c,stroke-width:2px,color:#0f172a;
    classDef dm fill:#fef3c7,stroke:#d97706,stroke-width:1px,color:#0f172a;
    classDef result fill:#ecfdf5,stroke:#059669,stroke-width:1px,color:#0f172a;
    classDef warn fill:#fee2e2,stroke:#dc2626,stroke-width:1px,color:#0f172a;

    class A1,A2,A3 grow;
    class B0,B1,B2,B3 shrink;
    class C1,C2,C3 ioctl;
    class D1,D2,D3 dm;
    class A4,B4 result;
    class B5 warn;
```

图 7 说明：

- 扩容路径是 `lvextend` 先修改 LVM 元数据，生成更大的 DM table，再通过 `DM_TABLE_LOAD` 装载 inactive table，resume 后成为新的 active table；之后 `resize2fs` 才能让文件系统使用新增空间。
- 缩容路径必须反过来：先删除扩容区测试文件并重写 md5，确保尾部有效数据已经移除，再用 `resize2fs` 缩小文件系统，最后执行 `lvreduce` 缩小 LV；否则 LV 尾部被裁掉后，文件系统仍引用旧范围，会产生数据丢失风险。
- DM 内核侧不直接原地修改旧 active table，而是装载新 table 后切换；切换完成后，table 范围、deps 和 backing lease 与新的 segment 布局一致。
- 对 linear / striped segment 组成的 table 来说，扩容可能扩大末段或追加 segment；缩容通常裁剪尾部范围，必要时删除末尾 segment；mixed table 表示同一张 table 同时包含 linear segment 和 striped segment。

## 图 8：Flush BIO 扇出、去重与完成聚合

```mermaid
flowchart TD
    subgraph A[一、Flush 来源]
        direction TB
        A1["文件系统刷盘<br/>sync / fsync"]
        A2["mapper 块设备<br/>/dev/mapper/name"]
        A3["Flush BIO<br/>无 sector remap"]
    end

    subgraph B[二、DM 数据面]
        direction TB
        B1["检查 active table<br/>设备必须可用"]
        B2["遍历 table deps<br/>收集 backing 设备"]
        B3["按设备去重<br/>同一 backing 只 flush 一次"]
    end

    subgraph C[三、底层块设备]
        direction TB
        C1["生成子 Flush<br/>每个 backing 一个"]
        C2["入队 backing<br/>submit flush"]
        C3["底层完成<br/>success / error"]
    end

    subgraph D[四、完成返回]
        direction TB
        D1["聚合完成结果"]
        D2["全部成功<br/>原 Flush 返回 Ok"]
        D3["任一失败<br/>原 Flush 返回 IoError"]
    end

    A1 --> A2 --> A3 --> B1
    B1 --> B2 --> B3 --> C1
    C1 --> C2 --> C3 --> D1
    D1 --> D2
    D1 --> D3

    classDef app fill:#e8f3ff,stroke:#2563eb,stroke-width:1px,color:#0f172a;
    classDef dm fill:#fff7ed,stroke:#ea580c,stroke-width:2px,color:#0f172a;
    classDef block fill:#f8fafc,stroke:#475569,stroke-width:1px,color:#0f172a;
    classDef result fill:#ecfdf5,stroke:#059669,stroke-width:1px,color:#0f172a;
    classDef error fill:#fee2e2,stroke:#dc2626,stroke-width:1px,color:#0f172a;

    class A1,A2,A3 app;
    class B1,B2,B3 dm;
    class C1,C2,C3 block;
    class D1,D2 result;
    class D3 error;
```

图 8 说明：

- Flush BIO 不像 Read/Write 那样按逻辑 sector 查找单个 segment，也不做 linear 或 striped sector remap；它表达的是“把此前写入持久化到底层设备”。
- DM 需要根据 active table 收集当前 table 依赖的 backing 设备，并对相同 backing 去重，避免 mixed table 或多 segment 场景下对同一底层设备重复 flush。
- 每个 backing 设备收到一个子 Flush；所有子 Flush 成功后，原 Flush 才返回成功；任一 backing flush 失败，原 Flush 返回 `IoError`。
- 这张图应与图 4 分开使用：图 4 解释 Read/Write 的 sector remap，图 8 解释 Flush 的 backing 扇出。

## 图 9：status / deps 查询与 backing 生命周期保护

```mermaid
flowchart TD
    subgraph A[一、用户态查询]
        direction TB
        A1["查询 table<br/>dmsetup table"]
        A2["查询 status<br/>dmsetup status"]
        A3["查询 deps<br/>dmsetup deps"]
        A4["LVM2 查询<br/>lvs --segments"]
    end

    subgraph B[二、DM ioctl]
        direction TB
        B1["DM_TABLE_STATUS<br/>返回 target 参数"]
        B2["DM_DEV_STATUS<br/>返回设备状态"]
        B3["DM_TABLE_DEPS<br/>返回 backing 依赖"]
    end

    subgraph C[三、DM 内核状态]
        direction TB
        C1["active table<br/>当前生效布局"]
        C2["target 实例<br/>linear / striped"]
        C3["deps 集合<br/>major:minor 列表"]
        C4["suspended / readonly<br/>设备标志"]
    end

    subgraph D[四、Asterinas 框架支撑]
        direction TB
        D1["block registry<br/>查找 backing 设备"]
        D2["BlockDeviceLease<br/>持有生命周期"]
        D3["devtmpfs / procfs<br/>暴露设备节点与设备号"]
    end

    subgraph E[五、最终作用]
        direction TB
        E1["用户可见布局<br/>验证 segment 形态"]
        E2["依赖不会悬空<br/>backing 不被提前释放"]
        E3["重建后可校验<br/>table/status/deps 一致"]
    end

    A1 --> B1 --> C1
    A2 --> B2 --> C4
    A3 --> B3 --> C3
    A4 --> A1

    C1 --> C2 --> C3
    C3 --> D1 --> D2 --> E2
    D3 --> A1
    D3 --> A2
    D3 --> A3

    C1 --> E1
    C3 --> E3
    C4 --> E3

    classDef user fill:#e8f3ff,stroke:#2563eb,stroke-width:1px,color:#0f172a;
    classDef ioctl fill:#fff7ed,stroke:#ea580c,stroke-width:2px,color:#0f172a;
    classDef dm fill:#fef3c7,stroke:#d97706,stroke-width:1px,color:#0f172a;
    classDef framework fill:#ecfdf5,stroke:#059669,stroke-width:1px,color:#0f172a;
    classDef result fill:#f8fafc,stroke:#475569,stroke-width:1px,color:#0f172a;

    class A1,A2,A3,A4 user;
    class B1,B2,B3 ioctl;
    class C1,C2,C3,C4 dm;
    class D1,D2,D3 framework;
    class E1,E2,E3 result;
```

图 9 说明：

- `dmsetup table` / `status` / `deps` 分别对应 table 参数、设备状态和 backing 依赖，是脚本判断 DM table 是否正确生效的重要观测面。
- `DM_TABLE_DEPS` 返回的是当前 active table 依赖的 backing 设备集合；这些依赖来自 linear / striped target 解析出的底层块设备。
- `BlockDeviceLease` 属于 Asterinas 内核框架支撑：DM table 持有 backing 设备期间，lease 防止 backing 设备生命周期提前结束。
- 重启恢复、扩容缩容、mixed table 校验都可以通过 `table/status/deps` 交叉验证：用户态看到的 segment 形态、DM active table、backing 依赖应保持一致。
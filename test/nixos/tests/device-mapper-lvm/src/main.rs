// SPDX-License-Identifier: MPL-2.0

//! Ordered LVM preparation: shared storage, then linear and striped ext2 workflows.
//!
//! Only shared preparation erases the two persistent raw disks. Each target grows
//! from 192 to 256 MiB and shrinks back, then leaves its own inactive LV and on-disk
//! checksum baselines for the recovery suite in a separate boot. Individual cases
//! require their documented predecessor state; they do not repair missing setup.

use nixos_test_framework::*;

nixos_test_main!(
    order = [
        "prepare_shared_lvm_storage",
        "linear_lv_offline_ext2_resize",
        "striped_lv_offline_ext2_resize",
    ]
);

// With 4 MiB extents, linear retains 48 extents on vde. Striped needs at most
// 32 extents on each PV, for a peak of (80, 32), below each 512 MiB PV's capacity.
// Check the actual extent counts instead of relying on aggregate VG free space.
// No other disk-writing suite or VM may intervene before both recovery cases.

#[nixos_test]
fn prepare_shared_lvm_storage(nixos_shell: &mut Session) -> Result<(), Error> {
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-lvm; test ! -L "$dm_work" && dm_nodes=$(find /dev -maxdepth 1 -name "dm-*" -print) && test -z "$dm_nodes" && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // Only this case destroys shared storage. Subsequent cases require its exact state.
    nixos_shell.run_cmd_and_expect_regex(
        r#"test -b /dev/vde && test "$(blockdev --getsize64 /dev/vde)" -eq 536870912"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test -b /dev/vdf && test "$(blockdev --getsize64 /dev/vdf)" -eq 536870912"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_sources=$(findmnt -rn -o SOURCE) && awk '$0 == "/dev/vde" || $0 == "/dev/vdf" || $0 ~ /^\/dev\/vd[ef][0-9]+$/ { mounted = 1 } END { exit mounted }' <<< "$dm_sources""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/zero of=/dev/vde bs=1M count=512 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/zero of=/dev/vdf bs=1M count=512 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"pvcreate --yes --dataalignment 1M /dev/vde /dev/vdf"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"vgcreate --physicalextentsize 4M phase1_offline_vg /dev/vde"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_vg=$(vgs --noheadings --units b --nosuffix -o vg_extent_size,pv_count phase1_offline_vg) && awk 'NF != 2 || $1 != 4194304 || $2 != 1 { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_vg""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_pv=$(pvs --noheadings -o pv_pe_count,pv_pe_alloc_count,vg_name /dev/vde) && awk 'NF != 3 || $1 !~ /^[0-9]+$/ || $1 < 80 || $2 != 0 || $3 != "phase1_offline_vg" { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_pv""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_pv=$(pvs --noheadings -o pv_pe_alloc_count,vg_name /dev/vdf) && awk 'NF != 1 || $1 != 0 { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_pv""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_pvs=$(pvs --noheadings --units s --nosuffix -o pv_name,pe_start /dev/vde /dev/vdf) && awk 'NF != 2 || ($1 != "/dev/vde" && $1 != "/dev/vdf") || seen[$1]++ || $2 != 2048 { bad = 1 } END { exit bad || NR != 2 }' <<< "$dm_pvs""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_lvs=$(lvs --noheadings -o lv_name phase1_offline_vg) && test -z "${dm_lvs//[[:space:]]/}""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    Ok(())
}

#[nixos_test]
fn linear_lv_offline_ext2_resize(nixos_shell: &mut Session) -> Result<(), Error> {
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-lvm; test ! -L "$dm_work" && dm_nodes=$(find /dev -maxdepth 1 -name "dm-*" -print) && test -z "$dm_nodes" && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_hex=$(stat -c "%t %T" /dev/vde) && set -- $dm_hex && test "$#" -eq 2 && printf -v dm_pv_a "%d:%d" "0x$1" "0x$2" && dm_hex=$(stat -c "%t %T" /dev/vdf) && set -- $dm_hex && test "$#" -eq 2 && printf -v dm_pv_b "%d:%d" "0x$1" "0x$2" && test "$dm_pv_a" != "$dm_pv_b""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_vg=$(vgs --noheadings --units b --nosuffix -o vg_extent_size,pv_count phase1_offline_vg) && awk 'NF != 2 || $1 != 4194304 || $2 != 1 { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_vg""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_pv=$(pvs --noheadings -o pv_pe_count,pv_pe_alloc_count,vg_name /dev/vde) && awk 'NF != 3 || $1 !~ /^[0-9]+$/ || $1 < 80 || $2 != 0 || $3 != "phase1_offline_vg" { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_pv""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_pv=$(pvs --noheadings -o pv_pe_alloc_count,vg_name /dev/vdf) && awk 'NF != 1 || $1 != 0 { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_pv""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_lvs=$(lvs --noheadings -o lv_name phase1_offline_vg) && test -z "${dm_lvs//[[:space:]]/}""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"lvcreate --type linear -an -L 192M -n ext2_lv phase1_offline_vg /dev/vde"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"lvchange -ay phase1_offline_vg/ext2_lv"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_lv=phase1_offline_vg/ext2_lv dm_name=phase1_offline_vg-ext2_lv dm_dev=/dev/dm-0; test -b "$dm_dev""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_size=$(blockdev --getsize64 "$dm_dev") && test "$dm_size" -eq 201326592"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_table=$(dmsetup table "$dm_name") && awk -v a="$dm_pv_a" -v b="$dm_pv_b" -v total=393216 '{ if (NF != 5 || $1 !~ /^[0-9]+$/ || $2 !~ /^[0-9]+$/ || $5 !~ /^[0-9]+$/ || $1 != next_sector || $2 <= 0 || $1 % 8192 || $2 % 8192 || $3 != "linear") bad = 1; if ($1 < 393216) { if ($1 + $2 > 393216 || $4 != a || $5 != 2048 + $1) bad = 1 } else { if ($4 != b || $5 != 2048 + $1 - 393216) bad = 1 } next_sector += $2 } END { exit bad || NR == 0 || next_sector != total }' <<< "$dm_table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_deps=$(dmsetup deps "$dm_name") && set -- $dm_deps && dm_deps="$*" && dm_dep_a="${dm_pv_a%:*}, ${dm_pv_a#*:}" && dm_dep_b="${dm_pv_b%:*}, ${dm_pv_b#*:}" && test "$dm_deps" = "1 dependencies : ($dm_dep_a)""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_pv=$(pvs --noheadings -o pv_pe_count,pv_pe_alloc_count,vg_name /dev/vde) && awk 'NF != 3 || $1 !~ /^[0-9]+$/ || $1 < 80 || $2 != 48 || $3 != "phase1_offline_vg" { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_pv""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_pv=$(pvs --noheadings -o pv_pe_alloc_count,vg_name /dev/vdf) && awk 'NF != 1 || $1 != 0 { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_pv""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // Check raw bytes before formatting; read failures and short reads must not pass cmp.
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="$dm_work/raw-expected" bs=512 count=512 iflag=fullblock status=none && test "$(stat -c %s "$dm_work/raw-expected")" -eq 262144"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="$dm_work/raw-expected" of="$dm_dev" bs=512 count=512 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="$dm_dev" of="$dm_work/raw-actual" bs=512 count=512 status=none && test "$(stat -c %s "$dm_work/raw-actual")" -eq 262144"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "$dm_work/raw-expected" "$dm_work/raw-actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"mkfs.ext2 -q -F -b 4096 -m 0 "$dm_dev""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_fs=$(dumpe2fs -h "$dm_dev" 2>/dev/null) && awk -F: '$1 == "Block count" { count++; if ($2 != 49152) bad = 1 } $1 == "Block size" { size++; if ($2 != 4096) bad = 1 } END { exit bad || count != 1 || size != 1 }' <<< "$dm_fs""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_mnt="$dm_work/linear_mnt"; mkdir -- "$dm_mnt""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"mount -t ext2 "$dm_dev" "$dm_mnt""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="$dm_mnt/retained" bs=1M count=48 iflag=fullblock conv=fsync status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test "$(stat -c %s "$dm_mnt/retained")" -eq 50331648"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"(cd "$dm_mnt" && sha256sum retained > retained.sha256)"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex(r#"umount "$dm_mnt""#, &Regex::new(r#"(?s).*"#).unwrap())?;

    // Prove the original filesystem survives remount before changing its capacity.
    nixos_shell.run_cmd_and_expect_regex(
        r#"e2fsck -f -n "$dm_dev""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"mount -t ext2 "$dm_dev" "$dm_mnt""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test "$(stat -c %s "$dm_mnt/retained")" -eq 50331648"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"(cd "$dm_mnt" && sha256sum -c retained.sha256)"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex(r#"umount "$dm_mnt""#, &Regex::new(r#"(?s).*"#).unwrap())?;

    // Grow the unmounted LV first, then ext2. Keep explicit target allocation constraints.
    nixos_shell.run_cmd_and_expect_regex(
        r#"vgextend phase1_offline_vg /dev/vdf"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_vg=$(vgs --noheadings --units b --nosuffix -o vg_extent_size,pv_count phase1_offline_vg) && awk 'NF != 2 || $1 != 4194304 || $2 != 2 { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_vg""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_pv=$(pvs --noheadings -o pv_pe_count,pv_pe_alloc_count,vg_name /dev/vde) && awk 'NF != 3 || $1 !~ /^[0-9]+$/ || $1 < 80 || $2 != 48 || $3 != "phase1_offline_vg" { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_pv""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_pv=$(pvs --noheadings -o pv_pe_count,pv_pe_alloc_count,vg_name /dev/vdf) && awk 'NF != 3 || $1 !~ /^[0-9]+$/ || $1 < 32 || $2 != 0 || $3 != "phase1_offline_vg" { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_pv""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"lvextend -L 256M "$dm_lv" /dev/vdf"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_size=$(blockdev --getsize64 "$dm_dev") && test "$dm_size" -eq 268435456"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_table=$(dmsetup table "$dm_name") && awk -v a="$dm_pv_a" -v b="$dm_pv_b" -v total=524288 '{ if (NF != 5 || $1 !~ /^[0-9]+$/ || $2 !~ /^[0-9]+$/ || $5 !~ /^[0-9]+$/ || $1 != next_sector || $2 <= 0 || $1 % 8192 || $2 % 8192 || $3 != "linear") bad = 1; if ($1 < 393216) { if ($1 + $2 > 393216 || $4 != a || $5 != 2048 + $1) bad = 1 } else { if ($4 != b || $5 != 2048 + $1 - 393216) bad = 1 } next_sector += $2 } END { exit bad || NR == 0 || next_sector != total }' <<< "$dm_table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_deps=$(dmsetup deps "$dm_name") && set -- $dm_deps && dm_deps="$*" && dm_dep_a="${dm_pv_a%:*}, ${dm_pv_a#*:}" && dm_dep_b="${dm_pv_b%:*}, ${dm_pv_b#*:}" && { test "$dm_deps" = "2 dependencies : ($dm_dep_a) ($dm_dep_b)" || test "$dm_deps" = "2 dependencies : ($dm_dep_b) ($dm_dep_a)"; }"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_pv=$(pvs --noheadings -o pv_pe_count,pv_pe_alloc_count,vg_name /dev/vde) && awk 'NF != 3 || $1 !~ /^[0-9]+$/ || $1 < 80 || $2 != 48 || $3 != "phase1_offline_vg" { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_pv""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_pv=$(pvs --noheadings -o pv_pe_count,pv_pe_alloc_count,vg_name /dev/vdf) && awk 'NF != 3 || $1 !~ /^[0-9]+$/ || $1 < 32 || $2 != 16 || $3 != "phase1_offline_vg" { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_pv""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"e2fsck -f -n "$dm_dev""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex(r#"resize2fs "$dm_dev""#, &Regex::new(r#"(?s).*"#).unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"e2fsck -f -n "$dm_dev""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_fs=$(dumpe2fs -h "$dm_dev" 2>/dev/null) && awk -F: '$1 == "Block count" { count++; if ($2 != 65536) bad = 1 } $1 == "Block size" { size++; if ($2 != 4096) bad = 1 } END { exit bad || count != 1 || size != 1 }' <<< "$dm_fs""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"mount -t ext2 "$dm_dev" "$dm_mnt""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test "$(stat -c %s "$dm_mnt/retained")" -eq 50331648"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"(cd "$dm_mnt" && sha256sum -c retained.sha256)"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // 208 MiB of non-sparse payload exceeds the original 192 MiB logical size.
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="$dm_mnt/added" bs=1M count=160 iflag=fullblock conv=fsync status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test "$(stat -c %s "$dm_mnt/added")" -eq 167772160"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"(cd "$dm_mnt" && sha256sum added > added.sha256)"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex(r#"umount "$dm_mnt""#, &Regex::new(r#"(?s).*"#).unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"mount -t ext2 "$dm_dev" "$dm_mnt""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test "$(stat -c %s "$dm_mnt/retained")" -eq 50331648"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test "$(stat -c %s "$dm_mnt/added")" -eq 167772160"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"(cd "$dm_mnt" && sha256sum -c retained.sha256 added.sha256)"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"rm -- "$dm_mnt/added" "$dm_mnt/added.sha256""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex(r#"umount "$dm_mnt""#, &Regex::new(r#"(?s).*"#).unwrap())?;

    // Shrink ext2 before the LV; --fs ignore avoids probes through absent VG/LV aliases.
    nixos_shell.run_cmd_and_expect_regex(
        r#"e2fsck -f -n "$dm_dev""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"resize2fs "$dm_dev" 192M"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_fs=$(dumpe2fs -h "$dm_dev" 2>/dev/null) && awk -F: '$1 == "Block count" { count++; if ($2 != 49152) bad = 1 } $1 == "Block size" { size++; if ($2 != 4096) bad = 1 } END { exit bad || count != 1 || size != 1 }' <<< "$dm_fs""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"lvreduce --fs ignore --yes -L 192M "$dm_lv""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_size=$(blockdev --getsize64 "$dm_dev") && test "$dm_size" -eq 201326592"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_table=$(dmsetup table "$dm_name") && awk -v a="$dm_pv_a" -v b="$dm_pv_b" -v total=393216 '{ if (NF != 5 || $1 !~ /^[0-9]+$/ || $2 !~ /^[0-9]+$/ || $5 !~ /^[0-9]+$/ || $1 != next_sector || $2 <= 0 || $1 % 8192 || $2 % 8192 || $3 != "linear") bad = 1; if ($1 < 393216) { if ($1 + $2 > 393216 || $4 != a || $5 != 2048 + $1) bad = 1 } else { if ($4 != b || $5 != 2048 + $1 - 393216) bad = 1 } next_sector += $2 } END { exit bad || NR == 0 || next_sector != total }' <<< "$dm_table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_deps=$(dmsetup deps "$dm_name") && set -- $dm_deps && dm_deps="$*" && dm_dep_a="${dm_pv_a%:*}, ${dm_pv_a#*:}" && dm_dep_b="${dm_pv_b%:*}, ${dm_pv_b#*:}" && test "$dm_deps" = "1 dependencies : ($dm_dep_a)""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_pv=$(pvs --noheadings -o pv_pe_count,pv_pe_alloc_count,vg_name /dev/vde) && awk 'NF != 3 || $1 !~ /^[0-9]+$/ || $1 < 80 || $2 != 48 || $3 != "phase1_offline_vg" { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_pv""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_pv=$(pvs --noheadings -o pv_pe_count,pv_pe_alloc_count,vg_name /dev/vdf) && awk 'NF != 3 || $1 !~ /^[0-9]+$/ || $1 < 32 || $2 != 0 || $3 != "phase1_offline_vg" { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_pv""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"e2fsck -f -n "$dm_dev""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"mount -t ext2 "$dm_dev" "$dm_mnt""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test "$(stat -c %s "$dm_mnt/retained")" -eq 50331648"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"(cd "$dm_mnt" && sha256sum -c retained.sha256)"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test ! -e "$dm_mnt/added" && test ! -e "$dm_mnt/added.sha256""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="$dm_mnt/after-shrink" bs=1M count=32 iflag=fullblock conv=fsync status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test "$(stat -c %s "$dm_mnt/after-shrink")" -eq 33554432"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"(cd "$dm_mnt" && sha256sum after-shrink > after-shrink.sha256)"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex(r#"umount "$dm_mnt""#, &Regex::new(r#"(?s).*"#).unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"mount -t ext2 "$dm_dev" "$dm_mnt""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test "$(stat -c %s "$dm_mnt/retained")" -eq 50331648"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test "$(stat -c %s "$dm_mnt/after-shrink")" -eq 33554432"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"(cd "$dm_mnt" && sha256sum -c retained.sha256 after-shrink.sha256)"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test ! -e "$dm_mnt/added" && test ! -e "$dm_mnt/added.sha256""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex(r#"umount "$dm_mnt""#, &Regex::new(r#"(?s).*"#).unwrap())?;

    // Preserve both disk metadata and original filesystem baselines for the next boot.
    nixos_shell.run_cmd_and_expect_regex(
        r#"lvchange -an "$dm_lv""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_lvs=$(lvs --binary --noheadings --units b --nosuffix -o lv_name,lv_size,lv_active phase1_offline_vg) && printf 'LV name/size/active (0 = inactive):\n%s\n' "$dm_lvs" && awk 'NF != 3 || !($1 == "ext2_lv") || seen[$1]++ || $2 != 201326592 || $3 !~ /^0$/ { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_lvs""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && dm_nodes=$(find /dev -maxdepth 1 -name "dm-*" -print) && test -z "$dm_nodes""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    Ok(())
}

#[nixos_test]
fn striped_lv_offline_ext2_resize(nixos_shell: &mut Session) -> Result<(), Error> {
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-lvm; test ! -L "$dm_work" && dm_nodes=$(find /dev -maxdepth 1 -name "dm-*" -print) && test -z "$dm_nodes" && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_hex=$(stat -c "%t %T" /dev/vde) && set -- $dm_hex && test "$#" -eq 2 && printf -v dm_pv_a "%d:%d" "0x$1" "0x$2" && dm_hex=$(stat -c "%t %T" /dev/vdf) && set -- $dm_hex && test "$#" -eq 2 && printf -v dm_pv_b "%d:%d" "0x$1" "0x$2" && test "$dm_pv_a" != "$dm_pv_b""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_vg=$(vgs --noheadings --units b --nosuffix -o vg_extent_size,pv_count phase1_offline_vg) && awk 'NF != 2 || $1 != 4194304 || $2 != 2 { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_vg""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_lvs=$(lvs --binary --noheadings --units b --nosuffix -o lv_name,lv_size,lv_active phase1_offline_vg) && printf 'LV name/size/active (0 = inactive):\n%s\n' "$dm_lvs" && awk 'NF != 3 || !($1 == "ext2_lv") || seen[$1]++ || $2 != 201326592 || $3 !~ /^0$/ { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_lvs""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_pv=$(pvs --noheadings -o pv_pe_count,pv_pe_alloc_count,vg_name /dev/vde) && awk 'NF != 3 || $1 !~ /^[0-9]+$/ || $1 < 80 || $2 != 48 || $3 != "phase1_offline_vg" { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_pv""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_pv=$(pvs --noheadings -o pv_pe_count,pv_pe_alloc_count,vg_name /dev/vdf) && awk 'NF != 3 || $1 !~ /^[0-9]+$/ || $1 < 32 || $2 != 0 || $3 != "phase1_offline_vg" { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_pv""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // At the striped peak, vde uses 48 + 32 extents and vdf uses 32.
    nixos_shell.run_cmd_and_expect_regex(
        r#"lvcreate --type striped --stripes 2 --stripesize 64 --alloc normal -an -L 192M -n striped_ext2_lv phase1_offline_vg /dev/vde /dev/vdf"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"lvchange -ay phase1_offline_vg/striped_ext2_lv"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_lv=phase1_offline_vg/striped_ext2_lv dm_name=phase1_offline_vg-striped_ext2_lv dm_dev=/dev/dm-0; test -b "$dm_dev""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_size=$(blockdev --getsize64 "$dm_dev") && test "$dm_size" -eq 201326592"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_table=$(dmsetup table "$dm_name") && awk -v a="$dm_pv_a" -v b="$dm_pv_b" -v total=393216 '{ if (NF != 9 || $1 !~ /^[0-9]+$/ || $2 !~ /^[0-9]+$/ || $7 !~ /^[0-9]+$/ || $9 !~ /^[0-9]+$/ || $1 != next_sector || $2 <= 0 || $2 % 16384 || $3 != "striped" || $4 != 2 || $5 != 128 || !(($6 == a && $8 == b) || ($6 == b && $8 == a))) bad = 1; for (i = 6; i <= 8; i += 2) { dev = $i; start = $(i+1); end = start + $2 / 2; if (start < 2048 || (start-2048) % 8192 || end > 1048576 || (dev == a && start < 395264)) bad = 1; for (j = 1; j <= count; j++) if (devices[j] == dev && start < ends[j] && starts[j] < end) bad = 1; count++; devices[count] = dev; starts[count] = start; ends[count] = end } next_sector += $2 } END { exit bad || NR == 0 || next_sector != total }' <<< "$dm_table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_deps=$(dmsetup deps "$dm_name") && set -- $dm_deps && dm_deps="$*" && dm_dep_a="${dm_pv_a%:*}, ${dm_pv_a#*:}" && dm_dep_b="${dm_pv_b%:*}, ${dm_pv_b#*:}" && { test "$dm_deps" = "2 dependencies : ($dm_dep_a) ($dm_dep_b)" || test "$dm_deps" = "2 dependencies : ($dm_dep_b) ($dm_dep_a)"; }"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_pv=$(pvs --noheadings -o pv_pe_count,pv_pe_alloc_count,vg_name /dev/vde) && awk 'NF != 3 || $1 !~ /^[0-9]+$/ || $1 < 80 || $2 != 72 || $3 != "phase1_offline_vg" { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_pv""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_pv=$(pvs --noheadings -o pv_pe_count,pv_pe_alloc_count,vg_name /dev/vdf) && awk 'NF != 3 || $1 !~ /^[0-9]+$/ || $1 < 32 || $2 != 24 || $3 != "phase1_offline_vg" { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_pv""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // Check raw bytes before formatting; read failures and short reads must not pass cmp.
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="$dm_work/raw-expected" bs=512 count=512 iflag=fullblock status=none && test "$(stat -c %s "$dm_work/raw-expected")" -eq 262144"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="$dm_work/raw-expected" of="$dm_dev" bs=512 count=512 conv=fsync,notrunc status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if="$dm_dev" of="$dm_work/raw-actual" bs=512 count=512 status=none && test "$(stat -c %s "$dm_work/raw-actual")" -eq 262144"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"cmp "$dm_work/raw-expected" "$dm_work/raw-actual""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"mkfs.ext2 -q -F -b 4096 -m 0 "$dm_dev""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_fs=$(dumpe2fs -h "$dm_dev" 2>/dev/null) && awk -F: '$1 == "Block count" { count++; if ($2 != 49152) bad = 1 } $1 == "Block size" { size++; if ($2 != 4096) bad = 1 } END { exit bad || count != 1 || size != 1 }' <<< "$dm_fs""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_mnt="$dm_work/striped_mnt"; mkdir -- "$dm_mnt""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"mount -t ext2 "$dm_dev" "$dm_mnt""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="$dm_mnt/retained" bs=1M count=48 iflag=fullblock conv=fsync status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test "$(stat -c %s "$dm_mnt/retained")" -eq 50331648"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"(cd "$dm_mnt" && sha256sum retained > retained.sha256)"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex(r#"umount "$dm_mnt""#, &Regex::new(r#"(?s).*"#).unwrap())?;

    // Prove the original filesystem survives remount before changing its capacity.
    nixos_shell.run_cmd_and_expect_regex(
        r#"e2fsck -f -n "$dm_dev""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"mount -t ext2 "$dm_dev" "$dm_mnt""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test "$(stat -c %s "$dm_mnt/retained")" -eq 50331648"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"(cd "$dm_mnt" && sha256sum -c retained.sha256)"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex(r#"umount "$dm_mnt""#, &Regex::new(r#"(?s).*"#).unwrap())?;

    // Grow the unmounted LV first, then ext2. Keep explicit target allocation constraints.
    nixos_shell.run_cmd_and_expect_regex(
        r#"lvextend --stripes 2 --stripesize 64 --alloc normal -L 256M "$dm_lv" /dev/vde /dev/vdf"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_size=$(blockdev --getsize64 "$dm_dev") && test "$dm_size" -eq 268435456"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_table=$(dmsetup table "$dm_name") && awk -v a="$dm_pv_a" -v b="$dm_pv_b" -v total=524288 '{ if (NF != 9 || $1 !~ /^[0-9]+$/ || $2 !~ /^[0-9]+$/ || $7 !~ /^[0-9]+$/ || $9 !~ /^[0-9]+$/ || $1 != next_sector || $2 <= 0 || $2 % 16384 || $3 != "striped" || $4 != 2 || $5 != 128 || !(($6 == a && $8 == b) || ($6 == b && $8 == a))) bad = 1; for (i = 6; i <= 8; i += 2) { dev = $i; start = $(i+1); end = start + $2 / 2; if (start < 2048 || (start-2048) % 8192 || end > 1048576 || (dev == a && start < 395264)) bad = 1; for (j = 1; j <= count; j++) if (devices[j] == dev && start < ends[j] && starts[j] < end) bad = 1; count++; devices[count] = dev; starts[count] = start; ends[count] = end } next_sector += $2 } END { exit bad || NR == 0 || next_sector != total }' <<< "$dm_table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_deps=$(dmsetup deps "$dm_name") && set -- $dm_deps && dm_deps="$*" && dm_dep_a="${dm_pv_a%:*}, ${dm_pv_a#*:}" && dm_dep_b="${dm_pv_b%:*}, ${dm_pv_b#*:}" && { test "$dm_deps" = "2 dependencies : ($dm_dep_a) ($dm_dep_b)" || test "$dm_deps" = "2 dependencies : ($dm_dep_b) ($dm_dep_a)"; }"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_pv=$(pvs --noheadings -o pv_pe_count,pv_pe_alloc_count,vg_name /dev/vde) && awk 'NF != 3 || $1 !~ /^[0-9]+$/ || $1 < 80 || $2 != 80 || $3 != "phase1_offline_vg" { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_pv""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_pv=$(pvs --noheadings -o pv_pe_count,pv_pe_alloc_count,vg_name /dev/vdf) && awk 'NF != 3 || $1 !~ /^[0-9]+$/ || $1 < 32 || $2 != 32 || $3 != "phase1_offline_vg" { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_pv""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"e2fsck -f -n "$dm_dev""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex(r#"resize2fs "$dm_dev""#, &Regex::new(r#"(?s).*"#).unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"e2fsck -f -n "$dm_dev""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_fs=$(dumpe2fs -h "$dm_dev" 2>/dev/null) && awk -F: '$1 == "Block count" { count++; if ($2 != 65536) bad = 1 } $1 == "Block size" { size++; if ($2 != 4096) bad = 1 } END { exit bad || count != 1 || size != 1 }' <<< "$dm_fs""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"mount -t ext2 "$dm_dev" "$dm_mnt""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test "$(stat -c %s "$dm_mnt/retained")" -eq 50331648"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"(cd "$dm_mnt" && sha256sum -c retained.sha256)"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // 208 MiB of non-sparse payload exceeds the original 192 MiB logical size.
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="$dm_mnt/added" bs=1M count=160 iflag=fullblock conv=fsync status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test "$(stat -c %s "$dm_mnt/added")" -eq 167772160"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"(cd "$dm_mnt" && sha256sum added > added.sha256)"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex(r#"umount "$dm_mnt""#, &Regex::new(r#"(?s).*"#).unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"mount -t ext2 "$dm_dev" "$dm_mnt""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test "$(stat -c %s "$dm_mnt/retained")" -eq 50331648"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test "$(stat -c %s "$dm_mnt/added")" -eq 167772160"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"(cd "$dm_mnt" && sha256sum -c retained.sha256 added.sha256)"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"rm -- "$dm_mnt/added" "$dm_mnt/added.sha256""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex(r#"umount "$dm_mnt""#, &Regex::new(r#"(?s).*"#).unwrap())?;

    // Shrink ext2 before the LV; --fs ignore avoids probes through absent VG/LV aliases.
    nixos_shell.run_cmd_and_expect_regex(
        r#"e2fsck -f -n "$dm_dev""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"resize2fs "$dm_dev" 192M"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_fs=$(dumpe2fs -h "$dm_dev" 2>/dev/null) && awk -F: '$1 == "Block count" { count++; if ($2 != 49152) bad = 1 } $1 == "Block size" { size++; if ($2 != 4096) bad = 1 } END { exit bad || count != 1 || size != 1 }' <<< "$dm_fs""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"lvreduce --fs ignore --yes -L 192M "$dm_lv""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_size=$(blockdev --getsize64 "$dm_dev") && test "$dm_size" -eq 201326592"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_table=$(dmsetup table "$dm_name") && awk -v a="$dm_pv_a" -v b="$dm_pv_b" -v total=393216 '{ if (NF != 9 || $1 !~ /^[0-9]+$/ || $2 !~ /^[0-9]+$/ || $7 !~ /^[0-9]+$/ || $9 !~ /^[0-9]+$/ || $1 != next_sector || $2 <= 0 || $2 % 16384 || $3 != "striped" || $4 != 2 || $5 != 128 || !(($6 == a && $8 == b) || ($6 == b && $8 == a))) bad = 1; for (i = 6; i <= 8; i += 2) { dev = $i; start = $(i+1); end = start + $2 / 2; if (start < 2048 || (start-2048) % 8192 || end > 1048576 || (dev == a && start < 395264)) bad = 1; for (j = 1; j <= count; j++) if (devices[j] == dev && start < ends[j] && starts[j] < end) bad = 1; count++; devices[count] = dev; starts[count] = start; ends[count] = end } next_sector += $2 } END { exit bad || NR == 0 || next_sector != total }' <<< "$dm_table""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_deps=$(dmsetup deps "$dm_name") && set -- $dm_deps && dm_deps="$*" && dm_dep_a="${dm_pv_a%:*}, ${dm_pv_a#*:}" && dm_dep_b="${dm_pv_b%:*}, ${dm_pv_b#*:}" && { test "$dm_deps" = "2 dependencies : ($dm_dep_a) ($dm_dep_b)" || test "$dm_deps" = "2 dependencies : ($dm_dep_b) ($dm_dep_a)"; }"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_pv=$(pvs --noheadings -o pv_pe_count,pv_pe_alloc_count,vg_name /dev/vde) && awk 'NF != 3 || $1 !~ /^[0-9]+$/ || $1 < 80 || $2 != 72 || $3 != "phase1_offline_vg" { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_pv""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_pv=$(pvs --noheadings -o pv_pe_count,pv_pe_alloc_count,vg_name /dev/vdf) && awk 'NF != 3 || $1 !~ /^[0-9]+$/ || $1 < 32 || $2 != 24 || $3 != "phase1_offline_vg" { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_pv""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"e2fsck -f -n "$dm_dev""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"mount -t ext2 "$dm_dev" "$dm_mnt""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test "$(stat -c %s "$dm_mnt/retained")" -eq 50331648"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"(cd "$dm_mnt" && sha256sum -c retained.sha256)"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test ! -e "$dm_mnt/added" && test ! -e "$dm_mnt/added.sha256""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dd if=/dev/urandom of="$dm_mnt/after-shrink" bs=1M count=32 iflag=fullblock conv=fsync status=none"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test "$(stat -c %s "$dm_mnt/after-shrink")" -eq 33554432"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"(cd "$dm_mnt" && sha256sum after-shrink > after-shrink.sha256)"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex(r#"umount "$dm_mnt""#, &Regex::new(r#"(?s).*"#).unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"mount -t ext2 "$dm_dev" "$dm_mnt""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test "$(stat -c %s "$dm_mnt/retained")" -eq 50331648"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test "$(stat -c %s "$dm_mnt/after-shrink")" -eq 33554432"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"(cd "$dm_mnt" && sha256sum -c retained.sha256 after-shrink.sha256)"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test ! -e "$dm_mnt/added" && test ! -e "$dm_mnt/added.sha256""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell
        .run_cmd_and_expect_regex(r#"umount "$dm_mnt""#, &Regex::new(r#"(?s).*"#).unwrap())?;

    // Preserve both disk metadata and original filesystem baselines for the next boot.
    nixos_shell.run_cmd_and_expect_regex(
        r#"lvchange -an "$dm_lv""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_lvs=$(lvs --binary --noheadings --units b --nosuffix -o lv_name,lv_size,lv_active phase1_offline_vg) && printf 'LV name/size/active (0 = inactive):\n%s\n' "$dm_lvs" && awk 'NF != 3 || !(($1 == "ext2_lv" || $1 == "striped_ext2_lv")) || seen[$1]++ || $2 != 201326592 || $3 !~ /^0$/ { bad = 1 } END { exit bad || NR != 2 }' <<< "$dm_lvs""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && dm_nodes=$(find /dev -maxdepth 1 -name "dm-*" -print) && test -z "$dm_nodes""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    Ok(())
}

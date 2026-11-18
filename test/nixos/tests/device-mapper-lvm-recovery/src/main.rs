// SPDX-License-Identifier: MPL-2.0

//! Independently discovers and verifies both retained LVs in a second VM boot.
//!
//! Requires the complete device-mapper-lvm preparation on the same two disks.
//! Each case activates only its own LV, mounts ext2 read-only, consumes the original
//! on-disk SHA256 manifests, then unmounts and deactivates without deleting data.
//! This tests clean-shutdown recovery, not autoactivation or sudden power loss.

use nixos_test_framework::*;

nixos_test_main!(
    order = [
        "linear_lv_recovers_after_reboot",
        "striped_lv_recovers_after_reboot",
    ]
);

#[nixos_test]
fn linear_lv_recovers_after_reboot(nixos_shell: &mut Session) -> Result<(), Error> {
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-lvm-recovery; test ! -L "$dm_work" && dm_nodes=$(find /dev -maxdepth 1 -name "dm-*" -print) && test -z "$dm_nodes" && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_hex=$(stat -c "%t %T" /dev/vde) && set -- $dm_hex && test "$#" -eq 2 && printf -v dm_pv_a "%d:%d" "0x$1" "0x$2" && dm_hex=$(stat -c "%t %T" /dev/vdf) && set -- $dm_hex && test "$#" -eq 2 && printf -v dm_pv_b "%d:%d" "0x$1" "0x$2" && test "$dm_pv_a" != "$dm_pv_b""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // Discover existing metadata; never prepare disks or regenerate checksum baselines.
    nixos_shell.run_cmd_and_expect_regex(r#"pvscan"#, &Regex::new(r#"(?s).*"#).unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(r#"vgscan"#, &Regex::new(r#"(?s).*"#).unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_vg=$(vgs --noheadings --units b --nosuffix -o vg_extent_size,pv_count phase1_offline_vg) && awk 'NF != 2 || $1 != 4194304 || $2 != 2 { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_vg""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_lvs=$(lvs --binary --noheadings --units b --nosuffix -o lv_name,lv_size,lv_active phase1_offline_vg) && printf 'LV name/size/active (0 = inactive):\n%s\n' "$dm_lvs" && awk 'NF != 3 || !(($1 == "ext2_lv" || $1 == "striped_ext2_lv")) || seen[$1]++ || $2 != 201326592 || $3 !~ /^0$/ { bad = 1 } END { exit bad || NR != 2 }' <<< "$dm_lvs""#,
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
        r#"test "$(blockdev --getsize64 /dev/vde)" -eq 536870912"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test "$(blockdev --getsize64 /dev/vdf)" -eq 536870912"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"lvchange -ay phase1_offline_vg/ext2_lv"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_lv=phase1_offline_vg/ext2_lv dm_name=phase1_offline_vg-ext2_lv; export dm_dev="/dev/$dm_lv"; test -L "$dm_dev" && test -L "/dev/mapper/$dm_name" && test -b "$dm_dev" && test "$dm_dev" -ef "/dev/mapper/$dm_name""#,
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
        r#"e2fsck -f -n "$dm_dev""#,
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
        r#"mount -t ext2 -o ro "$dm_dev" "$dm_mnt""#,
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

#[nixos_test]
fn striped_lv_recovers_after_reboot(nixos_shell: &mut Session) -> Result<(), Error> {
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_work=/tmp/aster-dm-lvm-recovery; test ! -L "$dm_work" && dm_nodes=$(find /dev -maxdepth 1 -name "dm-*" -print) && test -z "$dm_nodes" && dm_mounts=$(findmnt -rn -o TARGET) && awk -v work="$dm_work" '$0 == work || index($0, work "/") == 1 { mounted = 1 } END { exit mounted }' <<< "$dm_mounts" && rm -rf -- "$dm_work" && mkdir -- "$dm_work""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_hex=$(stat -c "%t %T" /dev/vde) && set -- $dm_hex && test "$#" -eq 2 && printf -v dm_pv_a "%d:%d" "0x$1" "0x$2" && dm_hex=$(stat -c "%t %T" /dev/vdf) && set -- $dm_hex && test "$#" -eq 2 && printf -v dm_pv_b "%d:%d" "0x$1" "0x$2" && test "$dm_pv_a" != "$dm_pv_b""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;

    // Discover existing metadata; never prepare disks or regenerate checksum baselines.
    nixos_shell.run_cmd_and_expect_regex(r#"pvscan"#, &Regex::new(r#"(?s).*"#).unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(r#"vgscan"#, &Regex::new(r#"(?s).*"#).unwrap())?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_vg=$(vgs --noheadings --units b --nosuffix -o vg_extent_size,pv_count phase1_offline_vg) && awk 'NF != 2 || $1 != 4194304 || $2 != 2 { bad = 1 } END { exit bad || NR != 1 }' <<< "$dm_vg""#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"dm_lvs=$(lvs --binary --noheadings --units b --nosuffix -o lv_name,lv_size,lv_active phase1_offline_vg) && printf 'LV name/size/active (0 = inactive):\n%s\n' "$dm_lvs" && awk 'NF != 3 || !(($1 == "ext2_lv" || $1 == "striped_ext2_lv")) || seen[$1]++ || $2 != 201326592 || $3 !~ /^0$/ { bad = 1 } END { exit bad || NR != 2 }' <<< "$dm_lvs""#,
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
        r#"test "$(blockdev --getsize64 /dev/vde)" -eq 536870912"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"test "$(blockdev --getsize64 /dev/vdf)" -eq 536870912"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"lvchange -ay phase1_offline_vg/striped_ext2_lv"#,
        &Regex::new(r#"(?s).*"#).unwrap(),
    )?;
    nixos_shell.run_cmd_and_expect_regex(
        r#"export dm_lv=phase1_offline_vg/striped_ext2_lv dm_name=phase1_offline_vg-striped_ext2_lv; export dm_dev="/dev/$dm_lv"; test -L "$dm_dev" && test -L "/dev/mapper/$dm_name" && test -b "$dm_dev" && test "$dm_dev" -ef "/dev/mapper/$dm_name""#,
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
        r#"e2fsck -f -n "$dm_dev""#,
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
        r#"mount -t ext2 -o ro "$dm_dev" "$dm_mnt""#,
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

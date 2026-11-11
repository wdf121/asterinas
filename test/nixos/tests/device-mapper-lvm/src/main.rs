// SPDX-License-Identifier: MPL-2.0

//! Single-boot LVM linear LV and ext2 workflow on a persistent raw disk.

use nixos_test_framework::*;

nixos_test_main!(order = ["linear_lv_single_boot", "linear_lv_live_replacement"]);

// This opt-in suite overwrites raw_test_1.img when invoked with raw_disk=1.
// Do not run it on a disk containing data to retain, or alongside another
// suite that expects the first raw disk's contents to remain intact.
const DISK: &str = "/dev/vde";
const VG: &str = "phase1_lvm_vg";
const LV: &str = "phase1_linear";

fn stage(shell: &mut Session, description: &str, body: &str, marker: &str) -> Result<(), Error> {
    // Keep LVM's device scan restricted to this suite's disk; the guest has no
    // udev, so do not wait for udev synchronization or mapper symlinks.
    // The PTY expects the command to be echoed verbatim. Raw multiline bodies
    // would trigger Bash's continuation prompt and break that echo handshake.
    let body = body.replace("\\\n", " ");
    // Keep commands below the interactive terminal's input limit. Write longer
    // bodies in bounded chunks and execute the script in the same shell as lvm().
    let body = if body.len() > 2048 {
        let path = "/tmp/phase1_lvm_stage.sh";
        let chars: Vec<char> = body.chars().collect();
        for (index, chunk) in chars.chunks(1024).enumerate() {
            let chunk: String = chunk.iter().collect();
            let quoted = chunk.replace('\'', "'\\''");
            let redirect = if index == 0 { ">" } else { ">>" };
            shell.run_cmd(&format!("printf %s '{quoted}' {redirect} {path}"))?;
        }
        format!(". {path}")
    } else {
        body
    };
    let command = format!(
        "bash -euo pipefail -c 'lvm() {{ local action=$1; shift; command lvm \"$action\" --config \"activation {{ udev_sync=0 udev_rules=0 }} devices {{ use_devicesfile=0 }}\" \"$@\"; }}; {body}; echo {marker}'"
    );
    shell.run_cmd_and_expect_display(description, &command, marker)
}

#[nixos_test]
fn linear_lv_single_boot(nixos_shell: &mut Session) -> Result<(), Error> {
    stage(
        nixos_shell,
        "verify the first raw disk before writing",
        &format!("test -b {DISK}; test \"$(blockdev --getsize64 {DISK})\" -eq 536870912"),
        "LVM_DISK_READY",
    )?;
    stage(
        nixos_shell,
        "clear the first raw disk",
        &format!("dd if=/dev/zero of={DISK} bs=1M count=512 conv=notrunc status=none; sync"),
        "LVM_DISK_CLEARED",
    )?;

    stage(
        nixos_shell,
        "initialize a physical volume",
        &format!("lvm pvcreate --yes --devices {DISK} {DISK}"),
        "LVM_PVCREATE_OK",
    )?;
    stage(
        nixos_shell,
        "create a volume group on that physical volume",
        &format!("lvm vgcreate --devices {DISK} {VG} {DISK}"),
        "LVM_VGCREATE_OK",
    )?;
    stage(
        nixos_shell,
        "create an inactive ordinary linear logical volume",
        &format!("lvm lvcreate --devices {DISK} --type linear -an -L 256M -n {LV} {VG}"),
        "LVM_LVCREATE_OK",
    )?;
    stage(
        nixos_shell,
        "activate the logical volume without udev",
        &format!("lvm lvchange --devices {DISK} -ay {VG}/{LV}"),
        "LVM_ACTIVATE_OK",
    )?;
    stage(
        nixos_shell,
        "inspect the single-PV linear mapping",
        &format!(
            "lvm pvs --devices {DISK} -o pv_name,vg_name; \
             lvm vgs --devices {DISK} -o vg_name,pv_count,lv_count; \
             lvm lvs --devices {DISK} --segments -o lv_name,seg_start,seg_size,segtype,devices {VG}/{LV}; \
             table=$(dmsetup table {VG}-{LV}); printf \"%s\\n\" \"$table\"; \
             test \"$(printf \"%s\\n\" \"$table\" | wc -l)\" -eq 1; \
             printf \"%s\\n\" \"$table\" | grep -qE \"^0 524288 linear [^[:space:]]+ [0-9]+$\"; \
             dmsetup deps -o devname {VG}-{LV} | grep -q vde"
        ),
        "LVM_LINEAR_TABLE_OK",
    )?;
    stage(
        nixos_shell,
        "read and write through the actual device-mapper primary node",
        &format!(
            "minor=$(dmsetup info -c --noheadings -o minor {VG}-{LV} | tr -d \" \" ); \
             test -n \"$minor\"; node=/dev/dm-$minor; test -b \"$node\"; \
             test \"$(blockdev --getsize64 \"$node\")\" -eq 268435456; \
             printf phase1-lvm-linear | dd of=\"$node\" bs=512 conv=sync,notrunc status=none; \
             printf phase1-lvm-linear | dd bs=512 conv=sync status=none | cmp - <(dd if=\"$node\" bs=512 count=1 status=none); \
             sync"
        ),
        "LVM_LINEAR_IO_OK",
    )?;
    stage(
        nixos_shell,
        "format and mount ext2 on the LVM logical volume",
        &format!(
            "minor=$(dmsetup info -c --noheadings -o minor {VG}-{LV} | tr -d \" \" ); \
             node=/dev/dm-$minor; test -b \"$node\"; \
             mkfs.ext2 -q -F -b 4096 -m 0 \"$node\"; \
             mkdir -p /tmp/phase1_lvm_mnt; \
             mount -t ext2 \"$node\" /tmp/phase1_lvm_mnt; \
             dd if=/dev/urandom of=/tmp/phase1_lvm_mnt/payload bs=1M count=64 conv=fsync status=none; \
             test \"$(stat -c %s /tmp/phase1_lvm_mnt/payload)\" -eq 67108864; \
             sha256sum /tmp/phase1_lvm_mnt/payload | cut -d \" \" -f1 > /tmp/phase1_lvm_payload.sha256; \
             sync; umount /tmp/phase1_lvm_mnt; \
             mount -t ext2 \"$node\" /tmp/phase1_lvm_mnt; \
             test \"$(stat -c %s /tmp/phase1_lvm_mnt/payload)\" -eq 67108864; \
             sha256sum /tmp/phase1_lvm_mnt/payload | cut -d \" \" -f1 | cmp - /tmp/phase1_lvm_payload.sha256; \
             umount /tmp/phase1_lvm_mnt"
        ),
        "LVM_EXT2_IO_OK",
    )?;
    stage(
        nixos_shell,
        "deactivate and remove the LV and VG before another test reuses the backing disk",
        &format!(
            "lvm lvchange --devices {DISK} -an {VG}/{LV}; \
                  lvm vgremove --devices {DISK} --yes {VG}; \
                  lvm pvremove --devices {DISK} --yes {DISK}"
        ),
        "LVM_DEACTIVATED_OK",
    )?;
    Ok(())
}

/// Exercise LVM-driven live table replacement with an active raw I/O workload.
/// Reuses the first raw disk after clearing it and also clears the second disk.
/// Neither test depends on data left by the other.
#[nixos_test]
fn linear_lv_live_replacement(nixos_shell: &mut Session) -> Result<(), Error> {
    stage(
        nixos_shell,
        "clear both disks owned by the live replacement test and create a linear LV",
        r#"test -b /dev/vde; test -b /dev/vdf; \
           test "$(blockdev --getsize64 /dev/vde)" -eq 536870912; \
           test "$(blockdev --getsize64 /dev/vdf)" -eq 536870912; \
           dd if=/dev/zero of=/dev/vde bs=1M count=512 conv=notrunc status=none; \
           dd if=/dev/zero of=/dev/vdf bs=1M count=512 conv=notrunc status=none; sync; \
           lvm pvcreate --yes --devices /dev/vde,/dev/vdf /dev/vde /dev/vdf; \
           lvm vgcreate --devices /dev/vde,/dev/vdf phase1_hot_vg /dev/vde; \
           lvm lvcreate --devices /dev/vde,/dev/vdf --type linear -an -L 128M -n hot_lv phase1_hot_vg; \
           lvm lvchange --devices /dev/vde,/dev/vdf -ay phase1_hot_vg/hot_lv; \
           minor=$(dmsetup info -c --noheadings -o minor phase1_hot_vg-hot_lv | tr -d " "); \
           test -n "$minor"; test -b "/dev/dm-$minor"; \
           printf "%s" "$minor" > /tmp/phase1_hot_minor; \
           test "$(blockdev --getsize64 "/dev/dm-$minor")" -eq 134217728; \
           test "$(dmsetup table phase1_hot_vg-hot_lv | wc -l)" -eq 1; \
           printf old-region | dd of="/dev/dm-$minor" bs=512 conv=sync,notrunc status=none; \
           dd if="/dev/dm-$minor" bs=512 count=1 status=none | grep -aq old-region"#,
        "LVM_HOT_INITIAL_OK",
    )?;
    stage(
        nixos_shell,
        "replace the LV table twice while raw reads and writes continue",
        r#"node=/dev/dm-$(cat /tmp/phase1_hot_minor); name=phase1_hot_vg-hot_lv; \
           stop=/tmp/phase1_hot_stop; progress=/tmp/phase1_hot_progress; \
           rm -f "$stop" "$progress"; : > "$progress"; \
           printf live-region | dd of=/tmp/phase1_hot_pattern bs=4096 conv=sync status=none; \
           ( while test ! -e "$stop"; do \
               dd if=/tmp/phase1_hot_pattern of="$node" bs=4096 seek=1024 conv=notrunc status=none || exit 1; \
               cmp /tmp/phase1_hot_pattern <(dd if="$node" bs=4096 skip=1024 count=1 status=none) || exit 1; \
               printf x >> "$progress"; \
             done ) & worker=$!; \
           trap "touch $stop; kill $worker 2>/dev/null || :" EXIT; \
           deadline=$((SECONDS + 30)); \
           while test "$(wc -c < "$progress")" -lt 5; do \
               test "$SECONDS" -lt "$deadline"; kill -0 "$worker"; sleep 0.1; \
           done; \
           before=$(wc -c < "$progress"); \
           lvm lvextend --devices /dev/vde,/dev/vdf -L 192M phase1_hot_vg/hot_lv /dev/vde; \
           deadline=$((SECONDS + 30)); \
           while test "$(wc -c < "$progress")" -le "$before"; do \
               test "$SECONDS" -lt "$deadline"; kill -0 "$worker"; sleep 0.1; \
           done; \
           test "$(cat /tmp/phase1_hot_minor)" = "$(dmsetup info -c --noheadings -o minor "$name" | tr -d " ")"; \
           test "$(blockdev --getsize64 "$node")" -eq 201326592; \
           test "$(dmsetup table "$name" | wc -l)" -eq 1; \
           lvm vgextend --devices /dev/vde,/dev/vdf phase1_hot_vg /dev/vdf; \
           before=$(wc -c < "$progress"); \
           lvm lvextend --devices /dev/vde,/dev/vdf -L 256M phase1_hot_vg/hot_lv /dev/vdf; \
           deadline=$((SECONDS + 30)); \
           while test "$(wc -c < "$progress")" -le "$before"; do \
               test "$SECONDS" -lt "$deadline"; kill -0 "$worker"; sleep 0.1; \
           done; \
           test "$(blockdev --getsize64 "$node")" -eq 268435456; \
           test "$(dmsetup table "$name" | wc -l)" -eq 2; \
           dmsetup deps -o devname "$name" | grep -q vde; \
           dmsetup deps -o devname "$name" | grep -q vdf; \
           printf cross-pv | dd of="$node" bs=512 seek=393215 conv=sync,notrunc status=none; \
           printf second-pv | dd of="$node" bs=512 seek=393216 conv=sync,notrunc status=none; \
           dd if="$node" bs=512 skip=393215 count=1 status=none | grep -aq cross-pv; \
           dd if="$node" bs=512 skip=393216 count=1 status=none | grep -aq second-pv; \
           second_offset=$(dmsetup table "$name" | awk "NR == 2 { print \$5 }"); \
           test -n "$second_offset"; \
           dd if=/dev/vdf bs=512 skip="$second_offset" count=1 status=none | grep -aq second-pv; \
           before=$(wc -c < "$progress"); \
           lvm lvreduce --devices /dev/vde,/dev/vdf --yes -L 192M phase1_hot_vg/hot_lv; \
           deadline=$((SECONDS + 30)); \
           while test "$(wc -c < "$progress")" -le "$before"; do \
               test "$SECONDS" -lt "$deadline"; kill -0 "$worker"; sleep 0.1; \
           done; \
           test "$(blockdev --getsize64 "$node")" -eq 201326592; \
           test "$(dmsetup table "$name" | wc -l)" -eq 1; \
           test "$(cat /tmp/phase1_hot_minor)" = "$(dmsetup info -c --noheadings -o minor "$name" | tr -d " ")"; \
           touch "$stop"; wait "$worker"; trap - EXIT; \
           test "$(wc -c < "$progress")" -gt 5; \
           cmp /tmp/phase1_hot_pattern <(dd if="$node" bs=4096 skip=1024 count=1 status=none); \
           dd if="$node" bs=512 count=1 status=none | grep -aq old-region; \
           test "$(dd if="$node" bs=512 skip=393216 count=1 status=none | wc -c)" -eq 0; \
           dd if=/dev/vdf bs=512 skip="$second_offset" count=1 status=none | grep -aq second-pv; \
           lvm lvchange --devices /dev/vde,/dev/vdf -an phase1_hot_vg/hot_lv; \
           lvm vgremove --devices /dev/vde,/dev/vdf --yes phase1_hot_vg; \
           lvm pvremove --devices /dev/vde,/dev/vdf --yes /dev/vde /dev/vdf"#,
        "LVM_HOT_REPLACEMENT_OK",
    )?;
    Ok(())
}

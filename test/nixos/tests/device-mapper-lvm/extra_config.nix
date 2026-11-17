# SPDX-License-Identifier: MPL-2.0

{ pkgs, ... }:

{
  environment.systemPackages = with pkgs; [
    e2fsprogs
    lvm2
  ];

  environment.etc."lvm/lvm.conf".text = ''
    # Native AIO is not supported yet; select LVM's synchronous engine.
    global/use_aio = 0

    # The distribution disables udev. Let LVM manage its nodes and symlinks
    # itself, without waiting for udev notifications.
    activation {
      udev_rules = 0
      udev_sync = 0

      # Memory locking is not supported yet. Disabling this protection can
      # cause resize failures or I/O deadlocks under memory pressure while
      # devices are suspended; this is a test-suite workaround, not a fix.
      reserved_stack = 0
      reserved_memory = 0
    }
  '';
}

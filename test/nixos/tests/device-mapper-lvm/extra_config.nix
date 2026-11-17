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
  '';
}

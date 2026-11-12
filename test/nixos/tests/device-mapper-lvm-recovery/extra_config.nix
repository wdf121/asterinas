# SPDX-License-Identifier: MPL-2.0

{ pkgs, ... }:

{
  environment.systemPackages = with pkgs; [
    e2fsprogs
    lvm2
  ];
}

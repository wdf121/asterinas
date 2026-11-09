# SPDX-License-Identifier: MPL-2.0

{ pkgs, ... }:

{
  environment.systemPackages = with pkgs; [
    hello-asterinas
    gitMinimal
    e2fsprogs
    lvm2
  ];
}

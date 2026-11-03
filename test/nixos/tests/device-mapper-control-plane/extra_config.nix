# SPDX-License-Identifier: MPL-2.0

{ pkgs, ... }:

{
  environment.systemPackages = with pkgs; [
    hello-asterinas
    gitMinimal
    lvm2
  ];
}

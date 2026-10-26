# SPDX-License-Identifier: MPL-2.0

# Guest tools for the Device Mapper control-plane tests. Disposable disks and
# QEMU supervision are provided by the host-side suite runner.
{ pkgs, ... }:

{
  # The test configuration merger replaces the base package list, so retain
  # hello-asterinas and gitMinimal alongside the suite's dependencies.
  environment.systemPackages = with pkgs; [
    # Provided by the DM branch's overlay; upstreaming this suite also requires
    # making the disk locator available in the target package set.
    aster-test-disk-locator
    coreutils
    diffutils
    gawk
    gitMinimal
    gnugrep
    gnused
    hello-asterinas
    lvm2
    util-linux
  ];
}

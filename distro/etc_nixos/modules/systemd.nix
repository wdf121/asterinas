{
  config,
  lib,
  pkgs,
  ...
}:

{
  systemd.package = pkgs.aster_systemd;

  # TODO: The following services currently do not work and
  # may affect systemd startup or cause performance issues.
  # Enable them after they can run successfully.
  systemd.coredump.enable = false;
  systemd.oomd.enable = false;
  systemd.services.logrotate.enable = false;
  systemd.services.network-setup.enable = false;
  systemd.services.resolvconf.enable = false;
  systemd.services.systemd-random-seed.enable = false;
  services.timesyncd.enable = false;
  services.udev.enable = false;

  services.getty.autologinUser = "root";
  services.getty.loginProgram = "${pkgs.util-linux.bin}/bin/login";
  systemd.services."serial-getty@hvc0".enable = false;
  users.users.root = {
    shell = "${pkgs.bash}/bin/bash";
    hashedPassword = null;
  };

  systemd.targets.getty.wants = lib.mkForce [ "autovt@hvc0.service" ];

  systemd.settings.Manager = {
    LogLevel = "crit";
    ShowStatus = "no";
  };
}

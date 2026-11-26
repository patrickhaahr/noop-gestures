{ pkgs, lib, ... }:

{
  languages.rust.enable = true;

  env.HOST_PKG_CONFIG = "${pkgs.pkg-config.pkg-config}/bin/pkg-config";

  packages = [
    pkgs.ast-grep
    pkgs.dbus
    pkgs.jq
    pkgs.just
    pkgs.pkg-config
  ];

  git-hooks.hooks = {
    git-file-size-guard = {
      enable = true;
      name = "git-file-size-guard";
      entry = "bash tools/git-file-size-guard";
      pass_filenames = false;
      always_run = true;
    };
    check = {
      enable = true;
      name = "just check";
      entry = "${lib.getExe pkgs.just} check";
      pass_filenames = false;
      always_run = true;
    };
  };
}

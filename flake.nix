{
  description = "NOOP wrist gestures for Wayland and niri";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (
        pkgs:
        let
          noop-gestures = pkgs.rustPlatform.buildRustPackage {
            pname = "noop-gestures";
            version = "0.1.0";
            src = self;
            cargoLock.lockFile = ./Cargo.lock;
            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = [ pkgs.dbus ];
            meta = {
              description = "NOOP wrist gestures for Wayland and niri";
              homepage = "https://github.com/patrickhaahr/noop-gestures";
              license = pkgs.lib.licenses.mit;
              mainProgram = "noop-gestures";
              platforms = pkgs.lib.platforms.linux;
            };
          };
        in
        {
          inherit noop-gestures;
          default = noop-gestures;
        }
      );
    };
}

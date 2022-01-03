{
  description = "RTIC Scope development environment";

  inputs = {
    nixpkgs.url      = "github:nixos/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url  = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
      in
      with pkgs;
      {
        devShell = mkShell rec {
          buildInputs = [
            openssl
            pkgconfig
            (rust-bin.stable.latest.default.override {
              targets = [ "x86_64-unknown-linux-gnu" "thumbv7em-none-eabihf" ];
            })

            pkgconfig
            pkgs.libusb
            openssl
            fontconfig
            freetype
            expat

            vulkan-loader
            vulkan-tools
            wayland
            wayland-protocols
            libxkbcommon
            swiftshader
            mesa_noglu
            libGL_driver
          ] ++ (with xorg; [libX11 libXcursor libXrandr libXi]);

          shellHook = ''
              export LD_LIBRARY_PATH="$LD_LIBRRAY_PATH:${lib.makeLibraryPath buildInputs}"
          '';
        };
      }
    );
}

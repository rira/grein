{
  description = "Grein workspace flake";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        grein = pkgs.rustPlatform.buildRustPackage {
          pname = "grein";
          version = "0.1.0";
          src = ./.;
          cargoLock = {
            lockFile = ./Cargo.lock;
          };
        };
      in
      {
        packages.default = grein;

        apps = {
          default = {
            type = "app";
            program = "${grein}/bin/grein";
          };
        };
      }
    );
}
{
  description = "RLMonitor — Minecraft chat history and player statistics";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, rust-overlay, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forEachSystem = nixpkgs.lib.genAttrs systems;
      builds = forEachSystem (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ (import rust-overlay) ];
          };
          rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          rustPlatform = pkgs.makeRustPlatform {
            cargo = rustToolchain;
            rustc = rustToolchain;
          };
          frontend = pkgs.callPackage ./nix/frontend.nix { };
          package = pkgs.callPackage ./nix/package.nix { inherit frontend rustPlatform; };
        in
        {
          inherit
            pkgs
            rustToolchain
            frontend
            package
            ;
        }
      );
    in
    {
      packages = forEachSystem (system: {
        default = builds.${system}.package;
        rlmonitor = builds.${system}.package;
        frontend = builds.${system}.frontend;
      });

      apps = forEachSystem (
        system:
        let
          app = {
            type = "app";
            program = "${builds.${system}.package}/bin/rlmonitor";
            meta.description = "Run RLMonitor";
          };
        in
        {
          default = app;
          rlmonitor = app;
        }
      );

      devShells = forEachSystem (system: {
        default = import ./nix/shell.nix {
          inherit (builds.${system}) pkgs rustToolchain;
        };
      });

      checks = forEachSystem (system: {
        package = builds.${system}.pkgs.callPackage ./nix/smoke-test.nix {
          inherit (builds.${system}) package;
        };
      });

      formatter = forEachSystem (system: builds.${system}.pkgs.nixfmt);
    };
}

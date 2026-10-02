{ pkgs, rustToolchain }:
pkgs.mkShell {
  packages = [
    pkgs.pkg-config
    pkgs.nodejs
    pkgs.pnpm
    pkgs.nixfmt
    (rustToolchain.override {
      extensions = [
        "rust-src"
        "rust-analyzer"
        "clippy"
        "rustfmt"
      ];
    })
  ];
}

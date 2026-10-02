{
  lib,
  rustPlatform,
  frontend,
}:
let
  manifest = builtins.fromTOML (builtins.readFile ../Cargo.toml);
in
rustPlatform.buildRustPackage {
  pname = manifest.package.name;
  inherit (manifest.package) version;

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../src
      ../Cargo.toml
      ../Cargo.lock
      ../rust-toolchain.toml
    ];
  };
  cargoLock.lockFile = ../Cargo.lock;

  env.RLMONITOR_FRONTEND_DIR = "${frontend}";

  postInstall = ''
    mkdir -p "$out/share/rlmonitor"
    ln -s ${frontend} "$out/share/rlmonitor/frontend"
  '';

  meta = {
    description = "Minecraft chat history and player statistics monitor for RealityLink";
    homepage = "https://github.com/idkidknow/RLMonitor";
    mainProgram = "rlmonitor";
    platforms = lib.platforms.linux;
  };
}

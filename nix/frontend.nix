{
  lib,
  stdenvNoCC,
  nodejs,
  pnpm,
  fetchPnpmDeps,
  pnpmConfigHook,
}:
let
  manifest = builtins.fromJSON (builtins.readFile ../frontend/package.json);
  dependencyFiles = lib.fileset.unions [
    ../frontend/package.json
    ../frontend/pnpm-lock.yaml
    ../frontend/pnpm-workspace.yaml
  ];
  dependencySource = lib.fileset.toSource {
    root = ../frontend;
    fileset = dependencyFiles;
  };
in
stdenvNoCC.mkDerivation (finalAttrs: {
  pname = "rlmonitor-frontend";
  inherit (manifest) version;

  src = lib.fileset.toSource {
    root = ../frontend;
    fileset = lib.fileset.unions [
      ../frontend/src
      ../frontend/public
      ../frontend/index.html
      ../frontend/vite.config.ts
      ../frontend/tsconfig.json
      dependencyFiles
    ];
  };

  nativeBuildInputs = [
    nodejs
    pnpm
    pnpmConfigHook
  ];
  pnpmDeps = fetchPnpmDeps {
    inherit (finalAttrs) pname version;
    inherit pnpm;
    src = dependencySource;
    fetcherVersion = 4;
    hash = "sha256-pHu+kPMF8zABXEnWAat1PGbErt3f/OSSxirDn54sm/s=";
  };

  buildPhase = ''
    runHook preBuild
    pnpm run build
    runHook postBuild
  '';

  doCheck = true;
  checkPhase = ''
    runHook preCheck
    pnpm run test
    runHook postCheck
  '';

  installPhase = ''
    runHook preInstall
    mkdir -p "$out"
    cp -r dist/. "$out/"
    runHook postInstall
  '';

  meta = {
    description = "RLMonitor web frontend";
    homepage = "https://github.com/idkidknow/RLMonitor";
    platforms = lib.platforms.linux;
  };
})

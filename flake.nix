{
  description = "Musishark, a Linux-first YouTube Music player written in Rust";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
  };

  outputs = { self, nixpkgs, utils, crane }:
    {
        overlays.default = final: _prev: {
          musishark = self.packages.${final.stdenv.hostPlatform.system}.default;
        };
    } // utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };

        version = pkgs.lib.pipe (self + "/io.github.matko802.Musishark.metainfo.xml") [
          builtins.readFile
          (builtins.match ".*<releases>[^<]*<release version=\"([^\"]+)\"[^>]*>.*")
          builtins.head
        ];

        gstPlugins = with pkgs.gst_all_1; [
          gstreamer
          gst-plugins-base
          gst-plugins-good
          gst-plugins-bad
        ];

        runtimeTools = [ pkgs.yt-dlp pkgs.nodejs pkgs.ffmpeg ];

        rustyV8Version = "130.0.7";
        rustyV8Archive = pkgs.fetchurl {
          url = "https://github.com/denoland/rusty_v8/releases/download/v${rustyV8Version}/librusty_v8_release_${pkgs.stdenv.hostPlatform.rust.rustcTarget}.a.gz";
          hash = {
            x86_64-linux = "sha256-pkdsuU6bAkcIHEZUJOt5PXdzK424CEgTLXjLtQ80t10=";
            aarch64-linux = "sha256-vu/ns1q+53FZ98tVCWZmsHYwwRBH1fOnTdBlhjcpkVo=";
          }.${system};
        };

        craneLib = crane.mkLib pkgs;
        src = pkgs.lib.fileset.toSource {
          root = ./.;
          fileset = pkgs.lib.fileset.unions [
            (craneLib.fileset.commonCargoSources ./.)
            ./resources
            ./assets
            ./io.github.matko802.Musishark.metainfo.xml
          ];
        };

        commonArgs = {
          inherit src;
          pname = "musishark";
          inherit version;
          strictDeps = true;

          nativeBuildInputs = [
            pkgs.pkg-config
            pkgs.wrapGAppsHook4
            pkgs.glib
            pkgs.mold
          ];

          buildInputs = [
            pkgs.gtk4
            pkgs.libadwaita
            pkgs.webkitgtk_6_0
            pkgs.sqlite
            pkgs.glib-networking
            pkgs.openssl
          ] ++ gstPlugins;

          RUSTY_V8_ARCHIVE = rustyV8Archive;
          RUSTFLAGS = "-C link-arg=-fuse-ld=mold";

          doCheck = false;
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        musishark = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;

          postInstall = ''
            install -Dm644 ${self}/io.github.matko802.Musishark.desktop $out/share/applications/io.github.matko802.Musishark.desktop
            install -Dm644 ${self}/io.github.matko802.Musishark.metainfo.xml $out/share/metainfo/io.github.matko802.Musishark.metainfo.xml
            install -Dm644 ${self}/assets/icons/hicolor/512x512/apps/io.github.matko802.Musishark.png $out/share/icons/hicolor/512x512/apps/io.github.matko802.Musishark.png
            install -Dm644 ${self}/assets/icons/hicolor/512x512/apps/io.github.matko802.Musishark-dark.png $out/share/icons/hicolor/512x512/apps/io.github.matko802.Musishark-dark.png
            install -Dm644 ${self}/assets/icons/hicolor/512x512/apps/io.github.matko802.Musishark-light.png $out/share/icons/hicolor/512x512/apps/io.github.matko802.Musishark-light.png
            install -Dm644 ${self}/assets/icons/hicolor/symbolic/apps/io.github.matko802.Musishark-symbolic.svg $out/share/icons/hicolor/symbolic/apps/io.github.matko802.Musishark-symbolic.svg
          '';

          preFixup = ''
            gappsWrapperArgs+=(--prefix PATH : ${pkgs.lib.makeBinPath runtimeTools})
          '';

          meta = {
            description = "A modern, Linux-first YouTube Music player";
            homepage = "https://github.com/Matko802/musishark";
            license = pkgs.lib.licenses.gpl3Plus;
            mainProgram = "musishark";
            platforms = pkgs.lib.platforms.linux;
          };
        });
      in {
        packages.default = musishark;

        devShells.default = pkgs.mkShell {
          inputsFrom = [ musishark ];
          packages = [ pkgs.cargo pkgs.rustc pkgs.clippy pkgs.rustfmt ] ++ runtimeTools;
          GST_PLUGIN_SYSTEM_PATH_1_0 = pkgs.lib.makeSearchPathOutput "lib" "lib/gstreamer-1.0" gstPlugins;
        };
      }
    );
}

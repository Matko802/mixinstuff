{
  description = "Mixinstuff, a Linux-first YouTube Music player written in Rust";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
  };

  outputs = { self, nixpkgs, utils, crane }:
    {
        overlays.default = final: _prev: {
          mixinstuff = self.packages.${final.stdenv.hostPlatform.system}.default;
        };
    } // utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };

        version = pkgs.lib.pipe (self + "/io.github.matko802.Mixinstuff.metainfo.xml") [
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

        # yt-dlp is the fallback stream resolver and the downloader. It wants a
        # JavaScript runtime for YouTube's player code, and ffmpeg to convert.
        runtimeTools = [ pkgs.yt-dlp pkgs.nodejs pkgs.ffmpeg ];

        # The v8 crate under the PO-token minter downloads a prebuilt library in
        # its build script. The sandbox has no network, so it is fetched here and
        # handed over through RUSTY_V8_ARCHIVE. The version follows v8 in Cargo.lock.
        rustyV8Version = "130.0.7";
        rustyV8Archive = pkgs.fetchurl {
          url = "https://github.com/denoland/rusty_v8/releases/download/v${rustyV8Version}/librusty_v8_release_${pkgs.stdenv.hostPlatform.rust.rustcTarget}.a.gz";
          hash = {
            x86_64-linux = "sha256-pkdsuU6bAkcIHEZUJOt5PXdzK424CEgTLXjLtQ80t10=";
            aarch64-linux = "sha256-vu/ns1q+53FZ98tVCWZmsHYwwRBH1fOnTdBlhjcpkVo=";
          }.${system};
        };

        craneLib = crane.mkLib pkgs;
        # Only cargo sources + the gresource files build.rs compiles.
        # Anything else (docs, screenshots) no longer invalidates anything.
        src = pkgs.lib.fileset.toSource {
          root = ./.;
          fileset = pkgs.lib.fileset.unions [
            (craneLib.fileset.commonCargoSources ./.)
            ./resources
            ./assets
            ./io.github.matko802.Mixinstuff.metainfo.xml
          ];
        };

        commonArgs = {
          inherit src;
          pname = "mixinstuff";
          inherit version;
          strictDeps = true;

          nativeBuildInputs = [
            pkgs.pkg-config
            pkgs.wrapGAppsHook4
            # build.rs runs glib-compile-resources for the stylesheet and icons.
            pkgs.glib
            # Fast linker for the final binary.
            pkgs.mold
          ];

          buildInputs = [
            pkgs.gtk4
            pkgs.libadwaita
            pkgs.webkitgtk_6_0
            pkgs.sqlite
            pkgs.glib-networking
            # The ytmusicapi crate brings reqwest with native TLS, so openssl-sys builds too.
            pkgs.openssl
          ] ++ gstPlugins;

          RUSTY_V8_ARCHIVE = rustyV8Archive;
          RUSTFLAGS = "-C link-arg=-fuse-ld=mold";

          # The tests that matter need the network or a signed-in session.
          doCheck = false;
        };

        # Dependencies compiled once; source edits only rebuild our crates.
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        mixinstuff = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;

          postInstall = ''
            install -Dm644 ${self}/io.github.matko802.Mixinstuff.desktop $out/share/applications/io.github.matko802.Mixinstuff.desktop
            install -Dm644 ${self}/io.github.matko802.Mixinstuff.metainfo.xml $out/share/metainfo/io.github.matko802.Mixinstuff.metainfo.xml
            install -Dm644 ${self}/assets/icons/hicolor/scalable/apps/io.github.matko802.Mixinstuff.svg $out/share/icons/hicolor/scalable/apps/io.github.matko802.Mixinstuff.svg
            install -Dm644 ${self}/assets/icons/hicolor/symbolic/apps/io.github.matko802.Mixinstuff-symbolic.svg $out/share/icons/hicolor/symbolic/apps/io.github.matko802.Mixinstuff-symbolic.svg
          '';

          preFixup = ''
            gappsWrapperArgs+=(--prefix PATH : ${pkgs.lib.makeBinPath runtimeTools})
          '';

          meta = {
            description = "A modern, Linux-first YouTube Music player";
            homepage = "https://github.com/Matko802/mixinstuff";
            license = pkgs.lib.licenses.gpl3Plus;
            mainProgram = "mixinstuff";
            platforms = pkgs.lib.platforms.linux;
          };
        });
      in {
        packages.default = mixinstuff;

        devShells.default = pkgs.mkShell {
          inputsFrom = [ mixinstuff ];
          packages = [ pkgs.cargo pkgs.rustc pkgs.clippy pkgs.rustfmt ] ++ runtimeTools;
          # GStreamer finds its plugins through this outside a wrapped binary.
          GST_PLUGIN_SYSTEM_PATH_1_0 = pkgs.lib.makeSearchPathOutput "lib" "lib/gstreamer-1.0" gstPlugins;
        };
      }
    );
}

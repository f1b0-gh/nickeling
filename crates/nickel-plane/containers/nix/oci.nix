{ pkgs, rootfs }:
let
  image = pkgs.dockerTools.buildLayeredImage {
    name = "localhost/nickel-plane";
    tag = "${rootfs.profile.revision}-${pkgs.lib.concatStringsSep "-" rootfs.profile.adapters}";
    # The existing rootfs already embeds its complete closure under /usr/lib/nix.
    # Adding store paths again would replace /nix and duplicate the closure.
    includeStorePaths = false;
    extraCommands = "cp -a ${rootfs}/. .";
    config = {
      Cmd = [ "/usr/bin/init" ];
      StopSignal = "SIGRTMIN+3";
      Env = [ "PATH=/usr/bin" ];
      Labels."org.opencontainers.image.title" = "nickel-plane";
    };
  };
  quadlet = pkgs.runCommand "nickel-plane-quadlet" { nativeBuildInputs = [ pkgs.python3 ]; } ''
    mkdir -p "$out/nickel-plane.service.d"
    cp ${../deploy/10-container-id.conf} "$out/nickel-plane.service.d/10-container-id.conf"
    python3 - ${image} ${../deploy/nickel-plane.container.in} "$out" <<'PY'
    import hashlib, json, pathlib, sys, tarfile
    with tarfile.open(sys.argv[1]) as archive:
        manifest = json.load(archive.extractfile('manifest.json'))
        assert len(manifest) == 1
        config = archive.extractfile(manifest[0]['Config']).read()
    image_id = 'sha256:' + hashlib.sha256(config).hexdigest()
    output = pathlib.Path(sys.argv[3])
    (output / 'image-id').write_text(image_id + '\n')
    (output / 'nickel-plane.container').write_text(pathlib.Path(sys.argv[2]).read_text().replace('@IMAGE_ID@', image_id))
    PY
  '';
in { inherit image quadlet; }

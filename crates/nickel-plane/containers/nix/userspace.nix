{ pkgs }:
{
  # Adapted from Organistalt's shared userspace builder. Keep only the container
  # catalog here: no dependency on a bootable OS or a sibling checkout.
  select = names: map (name: {
    busybox = pkgs.busybox;
    ca-certificates = pkgs.cacert;
  }.${name}) names;
  closure = packages: pkgs.closureInfo { rootPaths = packages; };
  copyClosure = closure: ''
    while read -r path; do
      cp -a "$path" "$out/usr/lib/nix/store/"
    done < ${closure}/store-paths
  '';
}

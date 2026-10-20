#!/bin/sh
assets() {
    # Updated only after reviewing the release and its checksums.
    cat <<'ASSETS'
# BEGIN RELEASE ASSETS
client:x86_64-unknown-linux-gnu 6df1a338b162111e47e4d006f67f3909694d905fc6d8110a43220f66644798b8
client:x86_64-apple-darwin acf3dd4e26740df7e76fdf4edb13f9d0b43b6c85b50d0edb791eab277cff18ed
client:aarch64-apple-darwin 3ad172f61f73211f85d7e9ec315ecd6e3b4bfbdee4f7984efd3817e837771efc
server:x86_64-unknown-linux-gnu da1f2ce22f7096ee50cca4848a4a8d49e646c770a03745900c010ee77437ad53
# END RELEASE ASSETS
ASSETS
}

main() {
    version=0.4.12
}

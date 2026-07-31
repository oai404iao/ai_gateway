#!/bin/sh
set -eu

runtime_dir=/run/ai-gateway
secret_dir="$runtime_dir/secrets"
config_source=/run/config/ai-gateway.toml
config_target="$runtime_dir/config.toml"
spool_dir=/var/lib/ai-gateway/request-log-spool
image_edit_spool_dir=/var/lib/ai-gateway/image-edit-spool

case "${1:-}" in
    --version|-V)
        exec gosu ai-gateway /usr/local/bin/ai-gateway "$@"
        ;;
esac

install -d -m 0700 -o ai-gateway -g ai-gateway "$runtime_dir" "$secret_dir"
install -d -m 0750 -o ai-gateway -g ai-gateway "$spool_dir"
install -d -m 0700 -o ai-gateway -g ai-gateway "$image_edit_spool_dir"

if [ ! -f "$config_source" ]; then
    echo "ai-gateway: missing configuration mount at $config_source" >&2
    exit 1
fi
install -m 0400 -o ai-gateway -g ai-gateway "$config_source" "$config_target"

copy_secret() {
    name=$1
    source="/run/secrets/$name"
    target="$secret_dir/$name"
    if [ ! -f "$source" ]; then
        echo "ai-gateway: missing Docker secret $name" >&2
        exit 1
    fi
    install -m 0400 -o ai-gateway -g ai-gateway "$source" "$target"
}

copy_secret postgres_password
copy_secret console_jwt_private
copy_secret console_jwt_public

exec gosu ai-gateway /usr/local/bin/ai-gateway "$@"

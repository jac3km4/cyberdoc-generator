set windows-shell := ["powershell.exe", "-NoLogo", "-Command"]
set dotenv-load

DEFAULT_INPUT := join("C:\\", "Program Files (x86)", "Steam", "steamapps", "common", "Cyberpunk 2077", "r6", "cache", "final.redscripts.bk")

input         := env_var_or_default("INPUT", DEFAULT_INPUT)
output        := env_var("OUTPUT")

lint:
    cargo +nightly clippy --fix --allow-dirty --allow-staged
    cargo +nightly fix --allow-dirty --allow-staged
    cargo +nightly fmt

generate INPUT='' OUTPUT='':
    @$in = if ('{{INPUT}}'  -EQ '') { '{{input}}'  } else { '{{INPUT}}'  }; \
    $out = if ('{{OUTPUT}}' -EQ '') { '{{output}}' } else { '{{OUTPUT}}' }; \
    cargo run --release -- --input $in --output $out
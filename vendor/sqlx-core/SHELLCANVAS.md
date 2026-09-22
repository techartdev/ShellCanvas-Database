# SQLx 0.8.6 TCP transport patch

Source: sqlx-core 0.8.6 from crates.io (MIT OR Apache-2.0; licenses included).
Only src/net/mod.rs and src/net/socket/mod.rs differ from upstream.

The public PostgreSQL/MySQL options in this version do not separate the TCP
destination from the hostname used for TLS verification. set_tcp_proxy pins
one logical host/port to one loopback port for this connector process. This
changes only socket establishment: TLS/SNI and database options retain the
original hostname. A destination mismatch fails closed; there is no direct
network fallback. Each app connection starts a separate adapter process.

Remove this patch when upstream provides a supported custom TCP destination.
On upgrades, check the two file diff and the adapter TCP routing regression.

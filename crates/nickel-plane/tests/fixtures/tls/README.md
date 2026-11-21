# Synthetic TLS test fixtures

These certificates and `server-test-key.pem` exist only for the loopback TLS
tests. The private key is public test data; never deploy it or trust these CAs
outside a disposable test. No host or infrastructure credentials are included.

The two independent RSA-2048 CAs were generated with OpenSSL. Each uses a
`Nickeling synthetic test CA` common name, critical CA basic constraints, and
certificate-signing/CRL-signing key usage. `server.pem` is signed by `new-ca.pem`,
has serial 1, and contains DNS `localhost` and IP `127.0.0.1` subject alternatives.
It has critical `CA:FALSE`, digital-signature/key-encipherment usage and server
authentication extended usage. Certificates last 7300 days from generation.

Regenerate disposable CA keys with `openssl req -new -x509 -newkey rsa:2048
-noenc -days 7300`, specifying those extensions with `-addext`. Generate a fresh
server key and CSR using `openssl req -new -newkey rsa:2048 -noenc`; sign it with
`openssl x509 -req -CA new-ca.pem -CAkey <disposable-ca-key> -set_serial 1
-days 7300 -extfile <server-extensions>`. Keep only the CA certificates, server
certificate and server test key. CA signing keys are not test inputs.

The test invokes `openssl s_server -HTTP` on loopback, using only these fixtures.
OpenSSL must be available in the development/CI environment. It verifies failed
trust with the old CA, rejection of changed snapshots, and successful trust
after reconnection; it does not qualify a Kubernetes API server.

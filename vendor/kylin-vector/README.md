# Kylin Vector Engine build headers

This directory vendors the C++ headers used to compile lymem's narrow C ABI
bridge against `libkysdk-vector-engine-client.so.1` on Kylin Linux Desktop V11.
They match runtime package `libkysdk-vector-engine-client 1.2.0.0-0k0.7`.

The SDK headers originate from `libkysdk-vector-engine-client`, derived from
Milvus SDK C++, and are licensed under Apache-2.0. The bundled nlohmann JSON
headers are licensed under MIT.

Do not replace these headers with a newer `-dev` package without verifying the
C++ ABI against the installed runtime library. The `1.2.0.0-0k1.1` development
headers expose a different `ConnectParam` ABI from the `0k0.7` runtime.

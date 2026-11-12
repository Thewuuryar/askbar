## Container images

Written by hand; the base images are the ones pinned by digest in the `Dockerfile` and `compose.yaml`.

| Image | Contains | License |
| --- | --- | --- |
| `askbar-bootstrap`, `-server`, `-admin`, `-auditor`, `-model-gateway`, `-alerter` | Debian 12 (`debian:bookworm-slim`) and one Askbar binary. | Debian's packages are each under their own license; the license text of every installed package is under `/usr/share/doc/<package>/copyright` in the image. The Askbar binary is AGPL-3.0-or-later, with the Rust crates above. |
| `askbar-caddy` | [Caddy](https://caddyserver.com) 2 (the official image, with the Caddy binary rebuilt to add the Cloudflare DNS plugin) and [`caddy-dns/cloudflare`](https://github.com/caddy-dns/cloudflare). | Caddy: Apache-2.0. The plugin: MIT. The Go modules in the binary have their own licenses (`caddy build-info` lists them). |
| `askbar-cpa` | [CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI) (`eceasy/cli-proxy-api`, unchanged) and Askbar's supervisor (AGPL-3.0-or-later, with the Rust crates above). | CLIProxyAPI: MIT; its notice is below. |
| `python:3.12-slim` (the `dev` profile only) | Python, to run the mock model in `tests/e2e/mock_llm.py`. It is not part of a release or a normal start. | Python Software Foundation License. |

### CLIProxyAPI

Copied unchanged from the LICENSE file of the upstream tag `v8.0.51` (the version the Dockerfile pins), which is the license of the code in the `askbar-cpa` image.

```text
MIT License

Copyright (c) 2025-2005.9 Luis Pater
Copyright (c) 2025.9-present Router-For.ME

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

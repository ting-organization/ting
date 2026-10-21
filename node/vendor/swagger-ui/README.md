# Bundled Swagger UI

Static assets from the official `swagger-ui-dist` npm distribution, pinned to
5.33.0. The node embeds only the browser bundle and CSS, plus the upstream LICENSE
and NOTICE. No npm dependency installation or package scripts are used.

`manifest.json` records the exact registry tarball, verified SHA-512 package
integrity and SHA-256 digests of the extracted files. Preserve LICENSE and NOTICE
when updating this bundle. Local styling overrides live in `node/web/site.css`.

The initializer uses `/openapi.json` from the same node and disables online
validation. No CDN, external example server or installation telemetry is used.

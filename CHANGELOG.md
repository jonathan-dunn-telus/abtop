# Changelog

## [0.5.5-remix.1](https://github.com/raphberube/abtop/compare/v0.5.5-remix.0...v0.5.5-remix.1) (2026-10-15)


### Features

* add aggregate stats scope for live sessions ([1ab65f7](https://github.com/raphberube/abtop/commit/1ab65f79d4e34479bfc704f7833e6d81f0a4ce46))
* add live agent concurrency panel ([a44d9c2](https://github.com/raphberube/abtop/commit/a44d9c2647374bcf9c4eae15997a72bf6e686493))
* add model usage pane with responsive layouts ([24ac2b9](https://github.com/raphberube/abtop/commit/24ac2b971362a46b24631b7ad2b08feb91e7a0f7))
* add optional cache-read chart and reuse metrics ([9632920](https://github.com/raphberube/abtop/commit/96329206115068f6a6a7d2e5f08ff49257e0ffa8))
* display completed sessions progressively during startup ([79348b2](https://github.com/raphberube/abtop/commit/79348b2bfd7998fe5db6404d43d7b19591e0e832))
* separate token rate and context panes ([014f491](https://github.com/raphberube/abtop/commit/014f491c7c94b2707bd38d92d1efbadc3c65e627))
* show background work as its own session status ([8c99a48](https://github.com/raphberube/abtop/commit/8c99a48256018766f3f6c48fe18578778050b060))
* show per-model token rates in models pane ([563f61b](https://github.com/raphberube/abtop/commit/563f61bb55b532b3f1233e5ee58d419497e96f3b))
* show per-session token rates by category ([ffd3089](https://github.com/raphberube/abtop/commit/ffd3089df985c7f7dc0f13f371074487230b56c2))


### Bug Fixes

* align remix usage with token totals ([4873d24](https://github.com/raphberube/abtop/commit/4873d24e19807e603a26c435ed9abac3134a1b81))
* detect Codex usage resets across cache changes ([22174ce](https://github.com/raphberube/abtop/commit/22174ceb09c888a3b3c1d9ade3255b851426eaa7))
* exclude cache reads from token totals ([04d19d7](https://github.com/raphberube/abtop/commit/04d19d77ac70b66ef7c3d55aa58e14fbd2146d10))
* include nested Claude subagent usage ([650bddb](https://github.com/raphberube/abtop/commit/650bddb03ca740b99ad020e4bb422b140d71987a))
* match model usage colors to context gradient ([45f0a41](https://github.com/raphberube/abtop/commit/45f0a41adfc14e6b221636e4017334c1b53726e6))
* simplify cache-read label in tokens panel ([84596b0](https://github.com/raphberube/abtop/commit/84596b045512d24475f3c0a64c095612f88b4be6))
* stabilize Claude subagent ordering and test lifecycle ([2d67313](https://github.com/raphberube/abtop/commit/2d6731396a6eaeb7bb0a836e36d824c00aea4faa))


### Documentation

* document remix releases and downstream sync rules ([24ce9c9](https://github.com/raphberube/abtop/commit/24ce9c925645a2cf269c4e5e0df27bfb1262ad52))
* explain how token totals highlight spending drivers ([e09ebc2](https://github.com/raphberube/abtop/commit/e09ebc2774adcbd5d7c80f8c892d09ff64a1cea5))
* merge token accounting cost explanation ([d890575](https://github.com/raphberube/abtop/commit/d890575669824c6fe3cf1e176e75b491cf57ef74))
* promote remix features with screenshots ([#3](https://github.com/raphberube/abtop/issues/3)) ([85c7848](https://github.com/raphberube/abtop/commit/85c78484f4be8b75c73a54f656c0f362ce1f7dce))

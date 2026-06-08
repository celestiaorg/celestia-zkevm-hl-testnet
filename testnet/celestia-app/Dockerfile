# Builds a celestia-app "standalone" image whose x/zkism Groth16 verifier
# accepts SP1 v6 proofs (356-byte encoding: 4-byte vkey prefix +
# exit_code|vk_root|proof_nonce metadata + 256-byte gnark proof, with the v6
# 5-public-input wrap circuit).
#
# Upstream (v7 through v8/main) ships a verifier built for the SP1 v5 proof
# format (260 bytes, 2 public inputs), which rejects v6 proofs with "invalid
# proof length: expected 260, got 356". The fix lives on the celestia-app
# branch CELESTIA_APP_REF (default jonas/update-sp1-v6, based on main (v9));
# this image is built directly from that branch.
#
# NOTE: the branch must exist on CELESTIA_APP_REPO before building.
# zkism-v6-verifier.patch is kept in this dir as a record of the exact diff.
#
# Build (from repo root):  make docker-build-celestia
ARG BUILDER_IMAGE=docker.io/golang:1.26.2-alpine
ARG RUNTIME_IMAGE=docker.io/alpine:3.22
# The branch carries the SP1 v6 zkism verifier fix on top of main (v9).
# Override CELESTIA_APP_REPO/REF to build from a fork or a different branch.
ARG CELESTIA_APP_REPO=https://github.com/celestiaorg/celestia-app.git
ARG CELESTIA_APP_REF=jonas/update-sp1-v6

# Stage 1: build the celestia-appd binary directly from the branch.
# hadolint ignore=DL3006
FROM --platform=$BUILDPLATFORM ${BUILDER_IMAGE} AS builder
ARG CELESTIA_APP_REPO
ARG CELESTIA_APP_REF
ARG TARGETOS
ARG TARGETARCH
ENV CGO_ENABLED=0
ENV GO111MODULE=on
# hadolint ignore=DL3018
RUN apk update && apk add --no-cache gcc git linux-headers make musl-dev
WORKDIR /celestia-app
# Fetch the branch that contains the SP1 v6 verifier fix and build it.
RUN git clone --depth 1 --branch ${CELESTIA_APP_REF} ${CELESTIA_APP_REPO} .
# Build the standalone (no multiplexer) celestia-appd binary.
RUN go mod download
RUN CGO_ENABLED=${CGO_ENABLED} GOOS=${TARGETOS} GOARCH=${TARGETARCH} make build-standalone

# Stage 2: minimal runtime image (mirrors upstream docker/standalone.Dockerfile).
# hadolint ignore=DL3006
FROM ${RUNTIME_IMAGE} AS runtime
ARG UID=10001
ARG USER_NAME=celestia
ENV CELESTIA_APP_HOME=/home/${USER_NAME}/.celestia-app
# hadolint ignore=DL3018
RUN apk update && apk add --no-cache bash curl jq \
    && adduser ${USER_NAME} -D -g ${USER_NAME} -h ${CELESTIA_APP_HOME} -s /sbin/nologin -u ${UID}
COPY --from=builder /celestia-app/build/celestia-appd /bin/celestia-appd
COPY --from=builder --chown=${USER_NAME}:${USER_NAME} /celestia-app/docker/entrypoint.sh /opt/entrypoint.sh
USER ${USER_NAME}
WORKDIR ${CELESTIA_APP_HOME}
EXPOSE 1317 9090 26656 26657 26660 26661
ENTRYPOINT [ "/bin/bash", "/opt/entrypoint.sh" ]

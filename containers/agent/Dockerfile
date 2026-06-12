# Build context = a staged directory containing BOTH repos:
#   ctx/darkmatter/               (transport core, path dependency)
#   ctx/finite-chat-darkmatter/   (this repo)
# Staged by scripts/agent-container-e2e.sh.

FROM rust:1-trixie AS builder
WORKDIR /build
COPY darkmatter ./darkmatter
COPY finite-chat-darkmatter ./finite-chat-darkmatter
RUN cd finite-chat-darkmatter && cargo build --release -p finitechat-cli

FROM python:3.13-slim-trixie
RUN pip install --no-cache-dir hermes-agent
COPY --from=builder /build/finite-chat-darkmatter/target/release/finitechat-darkmatter /usr/local/bin/finitechat-darkmatter
COPY finite-chat-darkmatter/integrations/hermes/finite-platform /root/.hermes/plugins/finite
COPY finite-chat-darkmatter/containers/agent/echo_agent.py /opt/echo_agent.py
ENV FINITECHAT_HOME=/data/agent
# FINITE_SERVER_URL is provided at `container run` time (the host's vmnet
# gateway address, e.g. http://192.168.64.1:18787).
CMD ["python", "/opt/echo_agent.py"]

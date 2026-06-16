# Build context = a staged directory containing this repo:
#   ctx/finitechat/
# Staged by scripts/agent-container-e2e.sh.

FROM rust:1-trixie AS builder
WORKDIR /build
COPY finitechat ./finitechat
RUN cd finitechat && cargo build --release -p finitechat-cli

FROM python:3.13-slim-trixie
RUN pip install --no-cache-dir hermes-agent
COPY --from=builder /build/finitechat/target/release/finitechat /usr/local/bin/finitechat
COPY finitechat/integrations/hermes/finite-platform /root/.hermes/plugins/finite
COPY finitechat/containers/agent/echo_agent.py /opt/echo_agent.py
ENV FINITECHAT_HOME=/data/agent
# FINITE_SERVER_URL is provided at `container run` time (the host's vmnet
# gateway address, e.g. http://192.168.64.1:18787).
CMD ["python", "/opt/echo_agent.py"]

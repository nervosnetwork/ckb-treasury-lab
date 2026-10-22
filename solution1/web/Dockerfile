FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive \
    PATH="/root/.bun/bin:${PATH}" \
    CKB_VOTE_NETWORK=testnet

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl unzip \
    && curl -fsSL https://bun.sh/install | bash \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .

WORKDIR /app/web
RUN bun install --frozen-lockfile

EXPOSE 5173

ENV CKB_VOTE_NETWORK=testnet
CMD ["bun", "run", "dev"]

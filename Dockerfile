FROM rust:alpine AS builder

WORKDIR /usr/src/app
COPY . .

RUN apk update && \
	apk upgrade

RUN apk add --no-cache musl-dev

RUN cargo install --path .

FROM alpine:latest
COPY --from=builder /usr/local/cargo/bin/dearrowdiscordbot /usr/local/bin/dearrowdiscordbot

CMD [ "dearrowdiscordbot" ]
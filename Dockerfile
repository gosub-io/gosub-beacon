FROM docker.io/library/rust

# Build dependencies
# From the docs, minimal dependencies
RUN apt-get update \
	&& apt-get install -y git make \
	&& apt-get install -y libgtk-4-dev libglib2.0-dev libcairo2-dev libgdk-pixbuf-2.0-dev \
                 libpango1.0-dev libsqlite3-dev libssl-dev pkg-config \
                 clang libclang-dev libgl-dev libegl-dev libfontconfig-dev libfreetype-dev \
	&& rm -rf /var/lib/apt/*

ARG ENGINE_REMOTE=https://github.com/gosub-io/gosub-engine.git
ARG ENGINE_BRANCH=beacon
# Engine dependencies
RUN mkdir /gosub \
	&& cd /gosub \
	&& git clone --single-branch --branch="$ENGINE_BRANCH" "$ENGINE_REMOTE" gosub-engine
#COPY ../gosub-engine /gosub/gosub-engine

#ARG BEACON_REMOTE=https://github.com/gosub-io/gosub-beacon.git
#ARG BEACON_BRANCH=main
#RUN mkdir /gosub \
#	&& cd /gosub \
#	&& git clone --single-branch --branch="$BEACON_BRANCH" "$BEACON_REMOTE" gosub-beacon
COPY . /gosub/gosub-beacon

WORKDIR /gosub/gosub-beacon

RUN make build

RUN chmod ugo-rw,ugo+rX,u+w -R /gosub

ENTRYPOINT ["/gosub/gosub-beacon/target/debug/gosub-beacon-gtk"]


FROM docker.io/library/rust

# Build dependencies
# From the docs, minimal dependencies
RUN apt-get update \
	&& apt-get install -y git make \
	&& apt-get install -y libgtk-4-dev libglib2.0-dev libcairo2-dev libgdk-pixbuf-2.0-dev \
                 libpango1.0-dev libsqlite3-dev libssl-dev pkg-config \
                 clang libclang-dev libgl-dev libegl-dev libfontconfig-dev libfreetype-dev \
	&& rm -rf /var/lib/apt/*

# Engine dependencies
RUN mkdir /gosub \
	&& cd /gosub \
	&& git clone https://github.com/gosub-io/gosub-engine.git
#COPY ../gosub-engine /gosub/gosub-engine

#RUN mkdir /gosub \
#	&& cd /gosub \
#	&& git clone https://github.com/gosub-io/gosub-beacon.git
COPY . /gosub/gosub-beacon

WORKDIR /gosub/gosub-beacon

RUN make build

RUN chmod ugo-rw,ugo+rX,u+w -R /gosub

ENTRYPOINT ["/gosub/gosub-beacon/target/debug/gosub-beacon"]


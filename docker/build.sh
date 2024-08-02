
#!/bin/bash -eo pipefail

export LORIKEET_VERSION=`cargo run -- --version |sed 's/lorikeet //'`
export LORIKEET_DOCKER_VERSION=wwood/lorikeet:$LORIKEET_VERSION
echo "Building $LORIKEET_DOCKER_VERSION .."

# Remove cmake as is was causing mamba env install failure
# cp ../lorikeet.yml . && \
# sed -i 's/.*pip.*//' lorikeet.yml && \
# sed -i 's/.*cmake.*//' lorikeet.yml && \
sed 's/LORIKEET_VERSION/'$LORIKEET_VERSION'/g' Dockerfile.in > Dockerfile && \
DOCKER_BUILDKIT=1 docker build -t $LORIKEET_DOCKER_VERSION . && \
docker run -v `pwd`:`pwd` $LORIKEET_DOCKER_VERSION call --full-help && \
echo "Seems good - now you just need to 'docker push $LORIKEET_DOCKER_VERSION'"

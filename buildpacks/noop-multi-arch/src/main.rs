use libcnb::build::{BuildContext, BuildResult, BuildResultBuilder};
use libcnb::detect::{DetectContext, DetectResult, DetectResultBuilder};
use libcnb::generic::{GenericError, GenericMetadata, GenericPlatform};
use libcnb::{buildpack_main, Buildpack};

struct NoopBuildpackMultiArch;

impl Buildpack for NoopBuildpackMultiArch {
    type Platform = GenericPlatform;
    type Metadata = GenericMetadata;
    type Error = GenericError;

    fn detect(&self, _context: DetectContext<Self>) -> libcnb::Result<DetectResult, Self::Error> {
        DetectResultBuilder::fail().build()
    }

    fn build(&self, _context: BuildContext<Self>) -> libcnb::Result<BuildResult, Self::Error> {
        BuildResultBuilder::new().build()
    }
}

buildpack_main!(NoopBuildpackMultiArch);

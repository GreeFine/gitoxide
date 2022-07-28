use crate::OutputFormat;

pub struct Options {
    pub format: OutputFormat,
}

pub(crate) mod function {
    use super::Options;
    use crate::OutputFormat;
    use git_repository as git;
    use std::ffi::OsString;

    pub fn parse(
        repo: git::Repository,
        spec: OsString,
        mut out: impl std::io::Write,
        Options { format }: Options,
    ) -> anyhow::Result<()> {
        let spec = git::path::os_str_into_bstr(&spec)?;
        let spec = repo.rev_parse(spec)?.detach();

        match format {
            OutputFormat::Human => {
                if let Some((kind, from, to)) = spec.range() {
                    writeln!(&mut out, "{}", from)?;
                    writeln!(
                        &mut out,
                        "{}{}",
                        matches!(kind, git::revision::spec::Kind::RangeBetween)
                            .then(|| "^")
                            .unwrap_or_default(),
                        to
                    )?;
                    if matches!(kind, git::revision::spec::Kind::RangeBetween) {
                        writeln!(out, "^TBD: compute and display merge base hash")?;
                    }
                } else if let Some(rev) = spec.single() {
                    writeln!(&mut out, "{}", rev)?;
                }
            }
            #[cfg(feature = "serde1")]
            OutputFormat::Json => {
                serde_json::to_writer_pretty(&mut out, &spec)?;
            }
        }
        Ok(())
    }
}

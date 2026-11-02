//! Metadata donors stay bound to original loaded files, not palette/VS inputs.
use super::*;
#[derive(Default)]
pub(super) struct Prepared {
    sources: Vec<Source>,
    args: Vec<OsString>,
}
impl Prepared {
    pub async fn prepare(
        value: Option<Box<media_core::ImageExportMetadata>>,
        probe: &Path,
        cancel: &watch::Receiver<bool>,
    ) -> Result<Self, AppError> {
        let Some(value) = value else {
            return Ok(Self::default());
        };
        if value.inputs.is_empty() {
            return Err(error(
                "Image metadata requires the original loaded inputs.",
                Path::new(""),
            ));
        }
        let mut result = Self::default();
        let mut documents = Vec::new();
        for input in &value.inputs {
            check(cancel)?;
            let source = Source::open(Path::new(input))?;
            documents.push(crate::jobs::probe(probe, &source.path, cancel, None).await?);
            source.verify()?;
            result.sources.push(source);
        }
        result.args = super::implicit::metadata_args(&value.options, &documents)?;
        Ok(result)
    }
    pub fn verify(&self) -> Result<(), AppError> {
        for source in &self.sources {
            source.verify()?;
        }
        Ok(())
    }
    pub fn append(&self, arguments: &mut Vec<OsString>) -> Result<(), AppError> {
        self.verify()?;
        if self.sources.is_empty() {
            return Ok(());
        }
        let offset = arguments.iter().filter(|value| *value == "-i").count();
        let position = arguments
            .iter()
            .position(|value| value == "-filter_complex")
            .ok_or_else(|| error("Missing prepared image filter output.", Path::new("")))?;
        let mut inputs = Vec::new();
        for source in &self.sources {
            inputs.push(OsString::from("-i"));
            inputs.push(source.path.as_os_str().to_owned());
        }
        arguments.splice(position..position, inputs);
        let mut options = self.args.clone();
        for index in 0..options.len().saturating_sub(1) {
            let flag = options[index].to_string_lossy();
            if matches!(flag.as_ref(), "-map_metadata" | "-map_chapters") {
                if let Ok(input) = options[index + 1].to_string_lossy().parse::<usize>() {
                    options[index + 1] = (input + offset).to_string().into();
                }
            } else if flag == "-map_metadata:s:t" {
                let text = options[index + 1].to_string_lossy();
                let (input, suffix) = text.split_once(':').expect("validated attachment metadata");
                let input = input.parse::<usize>().expect("validated input");
                options[index + 1] = format!("{}:{suffix}", input + offset).into();
            }
        }
        arguments.extend(options);
        Ok(())
    }
}

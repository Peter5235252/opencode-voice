use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, SupportedStreamConfig};
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

pub struct Capture {
    _stream: cpal::Stream,
    consumer: HeapCons<f32>,
    sample_rate: u32,
    device_name: String,
}

impl Capture {
    pub fn open(device_query: Option<&str>) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = select_device(&host, device_query)?;
        let config = choose_config(&device)?;
        let stream_config = config.config();
        let sample_rate = config.sample_rate();
        let channels = config.channels() as usize;
        let sample_format = config.sample_format();
        let device_name = device.to_string();

        let capacity = (sample_rate as usize).saturating_mul(2).max(16_000);
        let (producer, consumer) = HeapRb::<f32>::new(capacity).split();

        let stream = match sample_format {
            SampleFormat::F32 => build_stream::<f32>(&device, stream_config, channels, producer)?,
            SampleFormat::F64 => build_stream::<f64>(&device, stream_config, channels, producer)?,
            SampleFormat::I8 => build_stream::<i8>(&device, stream_config, channels, producer)?,
            SampleFormat::I16 => build_stream::<i16>(&device, stream_config, channels, producer)?,
            SampleFormat::I32 => build_stream::<i32>(&device, stream_config, channels, producer)?,
            SampleFormat::U8 => build_stream::<u8>(&device, stream_config, channels, producer)?,
            SampleFormat::U16 => build_stream::<u16>(&device, stream_config, channels, producer)?,
            other => {
                return Err(format!(
                    "unsupported input sample format: {other}. Try another --device or set the default source in PipeWire."
                ));
            }
        };

        stream
            .play()
            .map_err(|err| format!("could not start audio input: {err}"))?;

        Ok(Self {
            _stream: stream,
            consumer,
            sample_rate,
            device_name,
        })
    }

    pub fn read(&mut self, out: &mut [f32]) -> usize {
        self.consumer.pop_slice(out)
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }
}

pub fn list_devices() -> Result<(), String> {
    let host = cpal::default_host();
    let devices = host
        .input_devices()
        .map_err(|err| format!("could not enumerate audio devices: {err}"))?;

    let mut found = false;
    for device in devices {
        if device.supports_input() {
            println!("{device}");
            found = true;
        }
    }

    if !found {
        println!("(no audio input devices found)");
    }

    Ok(())
}

fn select_device(host: &cpal::Host, query: Option<&str>) -> Result<cpal::Device, String> {
    if let Some(query) = query {
        let needle = query.to_lowercase();
        let mut matches = host
            .input_devices()
            .map_err(|err| format!("could not enumerate audio devices: {err}"))?
            .filter(|device| device.supports_input())
            .filter(|device| device.to_string().to_lowercase().contains(&needle));

        let device = matches.next().ok_or_else(|| {
            format!(
                "no input device matches {query:?}; use --list-devices to see the available names"
            )
        })?;

        if matches.next().is_some() {
            eprintln!("note: more than one input device matches {query:?}; using {device}");
        }

        Ok(device)
    } else {
        host.default_input_device()
            .ok_or_else(|| "no default audio input device found".to_string())
    }
}

fn choose_config(device: &cpal::Device) -> Result<SupportedStreamConfig, String> {
    if let Ok(ranges) = device.supported_input_configs() {
        let ranges: Vec<_> = ranges.collect();

        // Prefer 16 kHz mono: it is the native rate of the keyword model and
        // avoids resampling in the hot path when PipeWire can provide it.
        for format in [
            SampleFormat::F32,
            SampleFormat::I16,
            SampleFormat::I32,
            SampleFormat::F64,
            SampleFormat::U16,
            SampleFormat::I8,
            SampleFormat::U8,
        ] {
            for range in &ranges {
                if range.channels() == 1
                    && range.sample_format() == format
                    && range.contains_rate(16_000)
                {
                    return Ok((*range).with_sample_rate(16_000));
                }
            }
        }
    }

    device
        .default_input_config()
        .map_err(|err| format!("could not get an input configuration: {err}"))
}

fn build_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    mut producer: HeapProd<f32>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                for frame in data.chunks_exact(channels) {
                    let mono = frame
                        .iter()
                        .map(|sample| f32::from_sample(*sample))
                        .sum::<f32>()
                        / channels as f32;

                    if producer.try_push(mono.clamp(-1.0, 1.0)).is_err() {
                        break;
                    }
                }
            },
            |err| eprintln!("audio stream error: {err}"),
            None,
        )
        .map_err(|err| format!("could not open audio input: {err}"))
}

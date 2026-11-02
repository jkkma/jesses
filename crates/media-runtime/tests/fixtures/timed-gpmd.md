# Synthetic timed gpmd fixture

`timed-gpmd.mov` contains one binary-data track (`gpmd`) with two 20-byte
synthetic KLV samples at 0 and 0.5 seconds, each lasting 0.5 seconds. It contains
no device/user metadata and does not claim to be a GPS/device recording. It
qualifies opaque data mapping, timestamps and byte identity.

Generated locally with the accompanying `generate-gpmd.c` against system
FFmpeg 6.1.1 development libraries. The migration-qualified FFmpeg/FFprobe 9.0.1
pair independently reads both packets and their durations. The checked fixture
avoids adding a C compiler/development-library dependency to native runtime tests.

SHA-256: `120033dbc736c790557e915ceeda797c659b37d6f39076cd5d0076d0ccc7176a`

Reproduce with a C compiler and libavformat/libavcodec/libavutil pkg-config
flags, run the generator with the destination path, then inspect it with
FFprobe `-show_streams -show_packets -show_data_hash sha256`.

#!/usr/bin/env python3
"""Bounded, opt-in real-footage source Dolby qualification (no source mutation).

Consumes the retained stream-copy Dolby fixture and actual JobManager outputs.
Literal reference graphs and source-style seek/wrapper/mux commands are kept
separate from the Rust planner. Only private files below --output are created.
"""
import argparse
import array
import hashlib
import json
import math
from pathlib import Path
import subprocess

GPU = "setparams=field_mode=prog:range=tv:color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc,libplacebo=tonemapping=spline:peak_detect=1:apply_dolbyvision=1:format=yuv420p10le:colorspace=bt709:color_primaries=bt709:color_trc=bt709:range=tv,limiter=min=64:max=940:planes=1,limiter=min=64:max=960:planes=6,sidedata=mode=delete,setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709"
# Literal ToneMapConfig.cs AnchorNits266.667, Fmt(4000/266.667)==15,
# ClampFilters and seven supported HDR side-data deletes. Earlier receipts retain the historical100-nit reference;
# current source admission uses this graph.
SOURCE_CPU = "setparams=color_trc=smpte2084:color_primaries=bt2020:colorspace=bt2020nc,zscale=transfer=linear:npl=266.667,format=gbrpf32le,zscale=primaries=bt709,tonemap=tonemap=hable:peak=15,zscale=transfer=bt709:matrix=bt709:range=tv,format=gbrp16le,zscale=matrix=bt709:range=tv,format=yuv444p16le," + ",".join("sidedata=mode=delete:type="+kind for kind in ("MASTERING_DISPLAY_METADATA","CONTENT_LIGHT_LEVEL","DOVI_METADATA","DOVI_RPU_BUFFER","DYNAMIC_HDR_PLUS","DYNAMIC_HDR_VIVID","AMBIENT_VIEWING_ENVIRONMENT"))


def update_cpu(args):
    root=args.output.resolve(strict=True);destination=root/"qualification.json";results=json.loads(destination.read_text())
    source_audio=packets(args.ffprobe,args.source,"a:0")
    for name in ("cpu-base","auto-fallback"):
        request=json.loads((root/f"{name}-request.json").read_text());path=Path(request["source"]["outputPath"]);proof=checked_output(args.ffprobe,path)
        expected,command=literal_reference(args.ffmpeg,args.source,SOURCE_CPU,120)
        proof["independentPixels"]=compare(expected,pixels(args.ffmpeg,path,120));proof["referenceArgv"]=command
        assert proof["frames"]==245 and proof["firstVideoSeconds"]==0
        assert packets(args.ffprobe,path,"a:0")==source_audio
        proof["copiedEac3Packets"]={"count":len(source_audio),"payloadAndPtsDtsDurationExact":True}
        proof["sourceCpuCalibration"]={"anchorNits":266.667,"peak":15,"clamp":"gbrp16le,zscale=matrix=bt709:range=tv,yuv444p16le","legacy100NitGraphUsed":False}
        results["cases"][name]=proof
    for name in ("positive-origin","vfr-gap"):
        request=json.loads((root/f"av1an-{name}-request.json").read_text());source=Path(request["source"]["inputPath"]);path=Path(request["source"]["outputPath"])
        work=root/f"literal-source-av1an-{name}";reference=root/f"literal-source-av1an-{name}.mkv"
        params="--output-depth 10 --output-csp i420 --input-range tv --range tv --sar 1:1 --colorprim bt709 --transfer bt709 --colormatrix bt709 --chromaloc 0 --preset ultrafast --crf 23 --threads 2"
        command=[args.av1an,"-i",source,"-o",reference,"--temp",work,"--encoder","x264","--passes","1","--no-defaults","--concat","mkvmerge","--chunk-method","select","--split-method","none","--extra-split","480","--workers","2","--pix-format","yuv420p10le","--video-params",params,"--ffmpeg","-vf "+SOURCE_CPU,"--audio-params","-an -sn -dn -map -0:t?","--keep","--verbose","--log-file",root/f"literal-source-av1an-{name}.log","-n"]
        if not reference.exists():run(command)
        proof=checked_output(args.ffprobe,path);expected=checked_output(args.ffprobe,reference)
        assert proof["frames"]==expected["frames"]==240 and proof["firstVideoSeconds"]==expected["firstVideoSeconds"]==0
        native_video=packets(args.ffprobe,path,"v:0");reference_video=packets(args.ffprobe,reference,"v:0")
        assert native_video==reference_video,(name,"all original engine coded packet clocks/payloads must match")
        assert packets(args.ffprobe,path,"a:0")==source_audio
        queue=json.loads((work/"chunks.json").read_text());done=json.loads((work/"done.json").read_text())
        assert [(chunk["start_frame"],chunk["end_frame"])for chunk in queue]==[(0,240)],queue
        proof["independentPinnedEngine"]={"argv":[str(value)for value in command],"queueSpan":[0,240],"inputPath":str(source),"sourcePictureCount":240,"codedPacketsAndPtsDtsDurationExact":True,"completeDone":done}
        proof["copiedEac3Packets"]={"count":126,"payloadAndPtsDtsDurationExact":True}
        results["cases"]["av1an-"+name]=proof
    results["sourceCpuCorrection"]="Source workflow uses literal fork266.667-nit anchor/clamp; earlier100-nit CPU rows retained only in previous-coherent-cpu-qualification.json, not source-brightness evidence. Unaffected GPU/Spline rows are retained without rerunning."
    destination.write_text(json.dumps(results,indent=2)+"\n");print(json.dumps({name:results["cases"][name]for name in ("cpu-base","auto-fallback","av1an-positive-origin","av1an-vfr-gap")},indent=2))


def run(args, **kwargs):
    result = subprocess.run([str(x) for x in args], check=False, capture_output=True, **kwargs)
    if result.returncode:
        raise RuntimeError(f"Command failed {result.returncode}: {args!r}\n{result.stderr[-4000:]!r}")
    return result.stdout


def probe(tool, path, *args):
    return json.loads(run([tool, "-v", "error", *args, "-of", "json", path]))


def packets(tool, path, selector):
    return probe(tool, path, "-select_streams", selector, "-show_packets", "-show_data_hash", "sha256",
                 "-show_entries", "packet=pts_time,dts_time,duration_time,data_hash")["packets"]


def checked_output(tool, path):
    actual = probe(tool, path, "-show_streams", "-show_frames")
    streams = actual["streams"]
    video = next(s for s in streams if s["codec_type"] == "video")
    frames = [f for f in actual["frames"] if f["media_type"] == "video"]
    assert video["codec_name"] == "h264" and video["pix_fmt"] == "yuv420p10le", video
    assert [video[k] for k in ("color_primaries", "color_transfer", "color_space", "color_range")] == ["bt709", "bt709", "bt709", "tv"]
    for frame in frames:
        assert [frame.get(k) for k in ("color_primaries", "color_transfer", "color_space", "color_range")] == ["bt709", "bt709", "bt709", "tv"], frame
        assert not any(any(word in side.get("side_data_type", "") for word in ("DOVI", "Dolby", "Mastering", "Content light", "HDR")) for side in frame.get("side_data_list", [])), frame
    timestamps = [float(f["best_effort_timestamp_time"]) for f in frames]
    assert all(a < b for a, b in zip(timestamps, timestamps[1:])), timestamps
    return {"frames": len(frames), "size": [video["width"], video["height"]], "firstVideoSeconds": timestamps[0],
            "lastVideoSeconds": timestamps[-1], "pixelFormat": video["pix_fmt"], "color": "limited BT709 SDR", "completeDecode": True, "hdrSideDataRemoved": True}


def pixels(ffmpeg, path, index):
    return run([ffmpeg, "-v", "error", "-i", path, "-map", "0:v:0", "-vf", f"select=eq(n\\,{index}),scale=192:108", "-frames:v", "1", "-pix_fmt", "yuv420p10le", "-f", "rawvideo", "-"])


def compare(expected, actual):
    assert len(actual) == len(expected) == 192 * 108 * 3
    a, b = array.array("H", expected), array.array("H", actual)
    rms = math.sqrt(sum((x-y)**2 for x, y in zip(a,b))/len(a))
    assert rms < 14, rms
    return {"samples":len(a),"rmse10bit":rms,"maximumAbsolute10bit":max(abs(x-y)for x,y in zip(a,b)),"referenceSha256":hashlib.sha256(expected).hexdigest(),"actualSha256":hashlib.sha256(actual).hexdigest()}


def literal_reference(ffmpeg, source, graph, selected_index, prefix=(), suffix=(), complex_graph=False):
    # The fork writes Y4M without fps_mode=passthrough. Its default cadence
    # selection is observable; a rawvideo reference would skip that behavior.
    command = [str(ffmpeg), "-v", "error", "-nostdin"]
    if "libplacebo" in graph: command += ["-init_hw_device", "vulkan"]
    command += list(prefix) + ["-i", str(source)]
    command += ["-filter_complex",graph,"-map","[reference]"]if complex_graph else["-map","0:v:0","-vf",graph]
    command += ["-pix_fmt", "yuv420p10le"] + list(suffix) + ["-an", "-sn", "-dn", "-f", "yuv4mpegpipe", "-strict", "-1", "-"]
    # The consumer drains every frame (select does not stop decoding) while
    # retaining only one small representative reference image in memory.
    with subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE) as producer:
        consume = subprocess.run([str(ffmpeg), "-v", "error", "-i", "-", "-vf", f"select=eq(n\\,{selected_index}),scale=192:108", "-fps_mode", "passthrough", "-pix_fmt", "yuv420p10le", "-f", "rawvideo", "-"], stdin=producer.stdout, capture_output=True)
        producer.stdout.close()
        stderr = producer.stderr.read()
        status = producer.wait()
    assert status == consume.returncode == 0, (status, consume.returncode, stderr, consume.stderr)
    return consume.stdout, command


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ffmpeg",type=Path,required=True);parser.add_argument("--ffprobe",type=Path,required=True)
    parser.add_argument("--source",type=Path,required=True);parser.add_argument("--output",type=Path,required=True)
    parser.add_argument("--update-cpu",action="store_true");parser.add_argument("--av1an",type=Path)
    args=parser.parse_args()
    if args.update_cpu:
        assert args.av1an is not None;update_cpu(args);return
    root=args.output.resolve(strict=True)
    results={"source":str(args.source.resolve(strict=True)),"sourceSize":args.source.stat().st_size,"cases":{}}
    source_audio=packets(args.ffprobe,args.source,"a:0")
    for name in ("cpu-base","auto-fallback"):
        request=json.loads((root/f"{name}-request.json").read_text());path=Path(request["source"]["outputPath"]);proof=checked_output(args.ffprobe,path)
        expected,command=literal_reference(args.ffmpeg,args.source,SOURCE_CPU,120)
        proof["independentPixels"]=compare(expected,pixels(args.ffmpeg,path,120));proof["referenceArgv"]=command
        assert packets(args.ffprobe,path,"a:0")==source_audio
        proof["copiedEac3Packets"]={"count":len(source_audio),"payloadAndPtsDtsDurationExact":True}
        results["cases"][name]=proof
    for name,index in (("text",2),("bitmap",3)):
        request=json.loads((root/f"trim-{name}-request.json").read_text());source=Path(request["source"]["inputPath"])
        path=Path(request["source"]["outputPath"]);proof=checked_output(args.ffprobe,path)
        graph=GPU+",subtitles=filename="+str(root/"timed-caption.srt")if name=="text"else f"[0:v:0]{GPU}[picture];[picture][0:{index}]overlay=shortest=1:format=yuv420p10[reference]"
        expected,command=literal_reference(args.ffmpeg,source,graph,30,[],["-ss","1","-t","1"],name=="bitmap")
        proof["independentPixels"]=compare(expected,pixels(args.ffmpeg,path,30));proof["referenceArgv"]=command
        assert proof["frames"]==60
        assert packets(args.ffprobe,path,"a:0")==packets(args.ffprobe,root/"trim-source-exact.mkv","a:0")
        proof["copiedEac3Packets"]={"count":len(packets(args.ffprobe,path,"a:0")),"matchesIndependentlyQualifiedExactTrim":True}
        results["cases"][name]=proof
    legacy=root/"legacy-rpu-SDR.mkv";proof=checked_output(args.ffprobe,legacy)
    assert proof["frames"]==240 and abs(proof["firstVideoSeconds"]-0.076)<0.00001
    assert packets(args.ffprobe,legacy,"a:0")==source_audio
    results["cases"]["legacyClockRegression"]=proof
    flac=root/"source-flac-SDR.mkv";proof=checked_output(args.ffprobe,flac)
    pcm=lambda path:run([args.ffmpeg,"-v","error","-i",path,"-map","0:a:0","-c:a","pcm_s24le","-f","s24le","-"])
    original_pcm,converted_pcm=pcm(args.source),pcm(flac)
    assert original_pcm==converted_pcm and len(original_pcm)==193536*6*3
    proof["independentDecodedEac3ToFlac"]={"samplesPerChannel":193536,"sampleRate":48000,"channels":6,"allDecoded24bitPcmExact":True,"pcmSha256":hashlib.sha256(converted_pcm).hexdigest()}
    results["cases"]["sourceConvertedFlac"]=proof
    for name,prefix,suffix in (("inputSeek",["-ss","1"],["-t","1"]),("exact",[],["-ss","1","-t","1"]),("frames",[],["-ss","0.992","-t","1.009","-frames:v","60"])):
        path=root/f"trim-source-{name}.mkv";proof=checked_output(args.ffprobe,path)
        expected,command=literal_reference(args.ffmpeg,args.source,GPU,30,prefix,suffix)
        proof["independentPixels"]=compare(expected,pixels(args.ffmpeg,path,30));proof["referenceArgv"]=command
        assert proof["frames"]==60,proof
        audio_reference=root/f"literal-source-audio-{name}.mkv"
        seek,duration=("0.992","1.009")if name=="frames"else("1","1")
        audio_command=[args.ffmpeg,"-v","error","-n","-ss",seek,"-i",args.source,"-map","0:a:0","-c","copy","-t",duration,audio_reference]
        if not audio_reference.exists():run(audio_command)
        actual_audio=packets(args.ffprobe,path,"a:0")
        assert actual_audio==packets(args.ffprobe,audio_reference,"a:0")
        proof["copiedEac3Packets"]={"count":len(actual_audio),"literalSourceAudioArgv":[str(value)for value in audio_command],"allPayloadPtsDtsDurationExact":True}
        results["cases"][name]=proof
    source_frames=probe(args.ffprobe,args.source,"-select_streams","v:0","-show_frames")["frames"]
    source_payloads=[packet["data_hash"]for packet in packets(args.ffprobe,args.source,"v:0")]
    for name in ("rotate90","vfr-gap"):
        request=json.loads((root/f"{name}-request.json").read_text());source=Path(request["source"]["inputPath"])
        frames=probe(args.ffprobe,source,"-select_streams","v:0","-show_frames")["frames"]
        assert [packet["data_hash"]for packet in packets(args.ffprobe,source,"v:0")]==source_payloads
        assert packets(args.ffprobe,source,"a:0")==source_audio
        assert len(frames)==len(source_frames)==240
        for frame in frames:
            kinds={side.get("side_data_type")for side in frame.get("side_data_list",[])}
            assert {"Dolby Vision RPU Data","Dolby Vision Metadata"}<=kinds,kinds
        for original,frame in zip(source_frames,frames):
            pts=float(original["pts_time"]);expected=pts+(0.25 if name=="vfr-gap"and pts>=2 else 0)
            assert abs(float(frame["pts_time"])-expected)<0.00001,(name,original,frame)
        path=Path(request["source"]["outputPath"]);proof=checked_output(args.ffprobe,path)
        expected,command=literal_reference(args.ffmpeg,source,GPU,120)
        proof["independentPixels"]=compare(expected,pixels(args.ffmpeg,path,120));proof["referenceArgv"]=command
        proof["originalCompressedFramesRpuPreserved"]={"packets":240,"everyPacketPayloadExact":True,"everyDecodedFrameHasRpuAndParsedMetadata":True,"presentationClockTransform":"unchanged"if name=="rotate90"else"+250ms at PTS>=2s; strictly increasing"}
        assert packets(args.ffprobe,path,"a:0")==source_audio
        proof["copiedEac3Packets"]={"count":len(source_audio),"payloadAndPtsDtsDurationExact":True}
        results["cases"][name]=proof
    destination=root/"qualification.json";destination.write_text(json.dumps(results,indent=2)+"\n")
    print(json.dumps({name:{"frames":proof["frames"],"firstVideoSeconds":proof["firstVideoSeconds"],"rmse":proof.get("independentPixels",{}).get("rmse10bit")}for name,proof in results["cases"].items()},indent=2))


if __name__=="__main__":main()

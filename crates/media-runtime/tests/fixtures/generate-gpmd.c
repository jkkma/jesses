#include <libavformat/avformat.h>
#include <libavcodec/packet.h>
#include <libavutil/common.h>
#include <string.h>
#include <stdio.h>
int main(int argc,char **argv){
 if(argc!=2)return 2;AVFormatContext *out=NULL;int r=avformat_alloc_output_context2(&out,NULL,"mov",argv[1]);if(r<0||!out)return 3;
 AVStream *s=avformat_new_stream(out,NULL);s->time_base=(AVRational){1,1000};s->codecpar->codec_type=AVMEDIA_TYPE_DATA;s->codecpar->codec_id=AV_CODEC_ID_BIN_DATA;s->codecpar->codec_tag=MKTAG('g','p','m','d');
 if(avio_open(&out->pb,argv[1],AVIO_FLAG_WRITE)<0)return 4;if(avformat_write_header(out,NULL)<0)return 5;
 for(int i=0;i<2;i++){AVPacket *p=av_packet_alloc();av_new_packet(p,20);memcpy(p->data,"TESTc\x01\x00\x0cJesses data0",19);p->data[19]='1'+i;p->stream_index=s->index;p->pts=p->dts=i*500;p->duration=500;p->flags=AV_PKT_FLAG_KEY;r=av_interleaved_write_frame(out,p);av_packet_free(&p);if(r<0)return 6;}
 r=av_write_trailer(out);avio_closep(&out->pb);avformat_free_context(out);return r<0?7:0;
}

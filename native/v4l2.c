// SPDX-License-Identifier: MIT
/* ABI adapter only. Test patterns, analysis and orchestration live in Rust. */
#include <errno.h>
#include <signal.h>
#include <fcntl.h>
#include <poll.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <limits.h>
#include <sys/ioctl.h>
#include <linux/dma-heap.h>
#include <linux/dma-buf.h>
#include <sys/mman.h>
#include <unistd.h>
#include "sdi_av.h"
#define BUFS VIDEO_MAX_FRAME
#define REQUEST_BUFS 6
struct info { char driver[16], card[32], bus[32]; uint32_t caps, output; };
struct mode { uint32_t width,height,interlaced,total_lines; uint64_t num,den; uint32_t index,reduced; };
struct layout { uint32_t width,height,fourcc,stride,sizes[5]; };
struct frame { uint32_t index,sequence,flags,events; uint64_t timestamp; uint8_t *data[5]; uint32_t len[5]; };
struct device { int fd; unsigned type,memory,count,on; void *map[BUFS][5]; size_t len[BUFS][5]; struct layout layout; int dma_fd[BUFS][5]; unsigned cpu[BUFS][5]; };
static int call(int fd,unsigned long r,void *p) { int v; do { v=ioctl(fd,r,p); } while(v<0 && errno==EINTR); return v<0?-errno:v; }
int vv_probe(const char *path,struct info *i) {
 int fd=open(path,O_RDWR|O_NONBLOCK|O_CLOEXEC); if(fd<0)return -errno;
 struct v4l2_capability c={0}; int r=call(fd,VIDIOC_QUERYCAP,&c); close(fd); if(r<0)return r;
 memset(i,0,sizeof(*i)); memcpy(i->driver,c.driver,16); memcpy(i->card,c.card,32); memcpy(i->bus,c.bus_info,32);
 i->caps=c.capabilities & V4L2_CAP_DEVICE_CAPS ? c.device_caps:c.capabilities;
 i->output=!!(i->caps & V4L2_CAP_VIDEO_OUTPUT_MPLANE); return 0;
}
static void mode_of(const struct v4l2_dv_timings *t,struct mode *m) {
 const struct v4l2_bt_timings *b=&t->bt; m->width=b->width; m->height=b->height; m->interlaced=b->interlaced;
 m->total_lines=V4L2_DV_BT_FRAME_HEIGHT(b); m->num=b->pixelclock; m->den=(uint64_t)V4L2_DV_BT_FRAME_WIDTH(b)*m->total_lines;
 if(b->flags & V4L2_DV_FL_REDUCED_FPS) {m->num*=1000; m->den*=1001;}
 m->reduced=(b->flags & V4L2_DV_FL_REDUCED_FPS)?2:!!(b->flags & V4L2_DV_FL_CAN_REDUCE_FPS);
}
int vv_mode(const char *path,unsigned index,struct mode *m) {
 int fd=open(path,O_RDWR|O_NONBLOCK|O_CLOEXEC); if(fd<0)return -errno;
 struct v4l2_enum_dv_timings e={.index=index}; int r=call(fd,VIDIOC_ENUM_DV_TIMINGS,&e); close(fd);
 if(r>=0) {memset(m,0,sizeof(*m)); mode_of(&e.timings,m); m->index=index;} return r;
}
int vv_format(const char *path,unsigned index,unsigned output,uint32_t *f) {
 int fd=open(path,O_RDWR|O_NONBLOCK|O_CLOEXEC); if(fd<0)return -errno;
 struct v4l2_fmtdesc e={.index=index,.type=output?V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE:V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE};
 int r=call(fd,VIDIOC_ENUM_FMT,&e); close(fd); if(r>=0)*f=e.pixelformat; return r;
}
void vv_close(struct device *d) {
 if(!d)return;
 if(d->on)call(d->fd,VIDIOC_STREAMOFF,&d->type);
 for(unsigned i=0;i<BUFS;i++)for(unsigned p=0;p<5;p++)if(d->map[i][p]) {
  if(d->cpu[i][p]){struct dma_buf_sync sync={.flags=DMA_BUF_SYNC_END|DMA_BUF_SYNC_RW};call(d->dma_fd[i][p],DMA_BUF_IOCTL_SYNC,&sync);}
  if(d->memory!=V4L2_MEMORY_USERPTR)munmap(d->map[i][p],d->len[i][p]); else free(d->map[i][p]);
 }
 for(unsigned i=0;i<BUFS;i++)for(unsigned p=0;p<5;p++)if(d->dma_fd[i][p]>=0)close(d->dma_fd[i][p]);
 if(d->fd>=0)close(d->fd);
 free(d);
}
int vv_open(const char *path,unsigned output,int index,unsigned reduced,uint32_t fourcc,unsigned userptr,struct device **result,struct layout *l,struct mode *m) {
 *result=NULL; struct device *d=calloc(1,sizeof(*d)); if(!d)return -ENOMEM;
 for(unsigned i=0;i<BUFS;i++)for(unsigned p=0;p<5;p++)d->dma_fd[i][p]=-1;
 d->fd=open(path,O_RDWR|O_NONBLOCK|O_CLOEXEC); int r=-errno; if(d->fd<0)goto fail;
 d->type=output?V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE:V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
 d->memory=userptr==2?V4L2_MEMORY_DMABUF:userptr==1?V4L2_MEMORY_USERPTR:V4L2_MEMORY_MMAP;
 struct v4l2_dv_timings t={0};
 if(index>=0) {struct v4l2_enum_dv_timings e={.index=(unsigned)index}; r=call(d->fd,VIDIOC_ENUM_DV_TIMINGS,&e); t=e.timings; if(reduced)t.bt.flags|=V4L2_DV_FL_REDUCED_FPS;}
 else {r=call(d->fd,VIDIOC_QUERY_DV_TIMINGS,&t);}
 if(r<0)goto fail;
 if(!t.bt.width || !t.bt.height || !t.bt.pixelclock) {r=-EPROTO;goto fail;}
 r=call(d->fd,VIDIOC_S_DV_TIMINGS,&t); if(r<0)goto fail;
 memset(m,0,sizeof(*m)); mode_of(&t,m);
 struct v4l2_format f={.type=d->type}; r=call(d->fd,VIDIOC_G_FMT,&f); if(r<0)goto fail;
 f.fmt.pix_mp.pixelformat=fourcc; r=call(d->fd,VIDIOC_S_FMT,&f); if(r<0)goto fail;
 if(f.fmt.pix_mp.num_planes!=5 || f.fmt.pix_mp.pixelformat!=fourcc) {r=-EPROTO;goto fail;}
 l->width=f.fmt.pix_mp.width; l->height=f.fmt.pix_mp.height; l->fourcc=fourcc; l->stride=f.fmt.pix_mp.plane_fmt[0].bytesperline;
 for(unsigned p=0;p<5;p++)l->sizes[p]=f.fmt.pix_mp.plane_fmt[p].sizeimage;
 d->layout=*l;
 struct v4l2_event_subscription sub={.type=V4L2_EVENT_SOURCE_CHANGE}; call(d->fd,VIDIOC_SUBSCRIBE_EVENT,&sub);
 struct v4l2_requestbuffers req={.count=REQUEST_BUFS,.type=d->type,.memory=d->memory}; r=call(d->fd,VIDIOC_REQBUFS,&req); if(r<0)goto fail;
 if(!req.count || req.count>BUFS) {r=-EPROTO;goto fail;} d->count=req.count;
 for(unsigned i=0;i<d->count;i++) {
  struct v4l2_plane p[5]={0}; struct v4l2_buffer b={.index=i,.type=d->type,.memory=d->memory,.length=5,.m.planes=p};
  if(!userptr) {r=call(d->fd,VIDIOC_QUERYBUF,&b);if(r<0)goto fail; if(b.length!=5){r=-EPROTO;goto fail;}}
  for(unsigned j=0;j<5;j++) {
   d->len[i][j]=userptr?l->sizes[j]:p[j].length;
   if(!d->len[i][j]){r=-EPROTO;goto fail;}
   if(userptr==2) {
    int heap=open("/dev/dma_heap/system",O_RDONLY|O_CLOEXEC);if(heap<0){r=-errno;goto fail;}
    struct dma_heap_allocation_data a={.len=(d->len[i][j]+4095)&~4095UL,.fd_flags=O_RDWR|O_CLOEXEC};r=call(heap,DMA_HEAP_IOCTL_ALLOC,&a);close(heap);if(r<0)goto fail;d->dma_fd[i][j]=a.fd;
    void *v=mmap(NULL,d->len[i][j],PROT_READ|PROT_WRITE,MAP_SHARED,a.fd,0);if(v==MAP_FAILED){r=-errno;goto fail;}d->map[i][j]=v;
   } else if(userptr==1) {int e=posix_memalign(&d->map[i][j],4096,d->len[i][j]); if(e){r=-e;goto fail;} memset(d->map[i][j],0,d->len[i][j]);}
   else {void *a=mmap(NULL,d->len[i][j],PROT_READ|PROT_WRITE,MAP_SHARED,d->fd,p[j].m.mem_offset);if(a==MAP_FAILED){r=-errno;goto fail;}d->map[i][j]=a;}
  }
 }
 *result=d; return (int)d->count;
 fail: vv_close(d);return r;
}
int vv_buffer(struct device *d,unsigned index,struct frame *f) {
 if(index>=d->count)return -EINVAL;
 memset(f,0,sizeof(*f));f->index=index;
 for(unsigned p=0;p<5;p++){f->data[p]=d->map[index][p];f->len[p]=d->len[index][p];if(d->memory==V4L2_MEMORY_DMABUF && !d->cpu[index][p]){struct dma_buf_sync sync={.flags=DMA_BUF_SYNC_START|DMA_BUF_SYNC_RW};int r=call(d->dma_fd[index][p],DMA_BUF_IOCTL_SYNC,&sync);if(r<0)return r;d->cpu[index][p]=1;}}return 0;
}
int vv_queue(struct device *d,const struct frame *f) {
 if(f->index>=d->count)return -EINVAL;
 struct v4l2_plane p[5]={0};struct v4l2_buffer b={.index=f->index,.type=d->type,.memory=d->memory,.length=5,.m.planes=p};
 for(unsigned j=0;j<5;j++){if(f->len[j]>d->len[f->index][j])return -EOVERFLOW;p[j].length=d->len[f->index][j];p[j].bytesused=f->len[j];if(d->memory==V4L2_MEMORY_USERPTR)p[j].m.userptr=(unsigned long)d->map[f->index][j];if(d->memory==V4L2_MEMORY_DMABUF){p[j].m.fd=d->dma_fd[f->index][j];struct dma_buf_sync sync={.flags=DMA_BUF_SYNC_END|DMA_BUF_SYNC_RW};if(d->cpu[f->index][j]){int r=call(p[j].m.fd,DMA_BUF_IOCTL_SYNC,&sync);if(r<0)return r;d->cpu[f->index][j]=0;}}}
 return call(d->fd,VIDIOC_QBUF,&b);
}
int vv_start(struct device *d) {int r=call(d->fd,VIDIOC_STREAMON,&d->type);if(r>=0)d->on=1;return r;}
int vv_next(struct device *d,unsigned timeout,struct frame *f) {
 struct pollfd pfd={.fd=d->fd,.events=(d->type==V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE?POLLOUT:POLLIN)|POLLPRI};
 struct v4l2_plane p[5]={0};struct v4l2_buffer b={.type=d->type,.memory=d->memory,.length=5,.m.planes=p};
 struct timespec now;if(clock_gettime(CLOCK_MONOTONIC,&now)<0)return -errno;
 uint64_t deadline=(uint64_t)now.tv_sec*1000+now.tv_nsec/1000000+timeout;
 unsigned events=0;int r;
 for(;;){
  if(clock_gettime(CLOCK_MONOTONIC,&now)<0)return -errno;
  uint64_t ms=(uint64_t)now.tv_sec*1000+now.tv_nsec/1000000;
  if(ms>=deadline)return -ETIMEDOUT;
  uint64_t remaining=deadline-ms;
  r=poll(&pfd,1,remaining>INT_MAX?INT_MAX:(int)remaining);
  if(r<0){if(errno==EINTR)continue;return -errno;}if(!r)return -ETIMEDOUT;
  if(pfd.revents&POLLPRI){struct v4l2_event e;while(call(d->fd,VIDIOC_DQEVENT,&e)>=0)events++;}
  if(pfd.revents&POLLNVAL)return -EBADF;
  if(pfd.revents&POLLHUP)return -ENODEV;
  if(!(pfd.revents&(POLLIN|POLLOUT|POLLERR)))continue;
  r=call(d->fd,VIDIOC_DQBUF,&b);
  if(r==-EAGAIN)continue;
  if(r<0)return r;
  break;
 }
 if(b.index>=d->count || b.length!=5)return -EPROTO;
 r=vv_buffer(d,b.index,f);if(r<0)return r;f->sequence=b.sequence;f->flags=b.flags;f->events=events;f->timestamp=(uint64_t)b.timestamp.tv_sec*1000000000+(uint64_t)b.timestamp.tv_usec*1000;
 for(unsigned j=0;j<5;j++) {if(p[j].bytesused>d->len[b.index][j] || p[j].data_offset>p[j].bytesused)return -EPROTO;f->data[j]=(uint8_t *)d->map[b.index][j]+p[j].data_offset;f->len[j]=p[j].bytesused-p[j].data_offset;}
 return 0;
}

int vv_contract(const char *path,unsigned output) {
 int fd=open(path,O_RDWR|O_NONBLOCK|O_CLOEXEC);if(fd<0)return -errno;
 struct v4l2_format f={.type=output?V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE:V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE};
 int r=call(fd,VIDIOC_G_FMT,&f);
 if(r>=0){if(f.fmt.pix_mp.num_planes!=SDI_NUM_PLANES || f.fmt.pix_mp.plane_fmt[3].sizeimage<128)r=-EPROTO;for(unsigned p=0;p<5;p++)if(!f.fmt.pix_mp.plane_fmt[p].sizeimage)r=-EPROTO;}
 close(fd);return r;
}

static volatile sig_atomic_t stopped;
static void stop_handler(int sig) { (void)sig;stopped=1; }
void vv_signals(void) {struct sigaction sa={0};sa.sa_handler=stop_handler;sigemptyset(&sa.sa_mask);sigaction(SIGINT,&sa,NULL);sigaction(SIGTERM,&sa,NULL);}
int vv_stopped(void) {return stopped;}

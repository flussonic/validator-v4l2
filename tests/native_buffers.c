// SPDX-License-Identifier: MIT
/* Exercise vv_open when the driver raises REQBUFS to its queue minimum. */
#include <assert.h>
#include <stdarg.h>
#include <sys/mman.h>

static void *fake_mmap(void *addr, size_t length, int prot, int flags,
                       int fd, off_t offset)
{
    (void)flags;
    (void)fd;
    (void)offset;
    return mmap(addr, length, prot, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
}
#define mmap fake_mmap
#define ioctl fake_ioctl
#include "../native/v4l2.c"
#undef mmap
#undef ioctl

static unsigned returned_count = 16, queried;
int fake_ioctl(int fd, unsigned long request, ...)
{
    (void)fd;
    va_list args;
    va_start(args, request);
    void *arg = va_arg(args, void *);
    va_end(args);
    switch (request) {
    case VIDIOC_ENUM_DV_TIMINGS: {
        struct v4l2_enum_dv_timings *timings = arg;
        timings->timings.bt.width = 1920;
        timings->timings.bt.height = 1080;
        timings->timings.bt.pixelclock = 74250000;
        return 0;
    }
    case VIDIOC_S_DV_TIMINGS:
    case VIDIOC_SUBSCRIBE_EVENT:
        return 0;
    case VIDIOC_G_FMT:
    case VIDIOC_S_FMT: {
        struct v4l2_format *format = arg;
        format->fmt.pix_mp.pixelformat = v4l2_fourcc('S', 'D', 'U', 'Y');
        format->fmt.pix_mp.num_planes = 5;
        for (unsigned p = 0; p < 5; p++)
            format->fmt.pix_mp.plane_fmt[p].sizeimage = 128;
        return 0;
    }
    case VIDIOC_REQBUFS: {
        struct v4l2_requestbuffers *buffers = arg;
        assert(buffers->count == 6);
        buffers->count = returned_count;
        return 0;
    }
    case VIDIOC_QUERYBUF: {
        struct v4l2_buffer *buffer = arg;
        assert(buffer->index == queried++);
        for (unsigned p = 0; p < 5; p++)
            buffer->m.planes[p].length = 128;
        return 0;
    }
    default:
        assert(0);
        return -1;
    }
}

int main(void)
{
    struct device *dev;
    struct layout layout;
    struct mode mode;
    int count = vv_open("/dev/null", 1, 0, 0, v4l2_fourcc('S','D','U','Y'),
                        0, &dev, &layout, &mode);
    assert(count == 16 && queried == 16);
    struct frame frame;
    assert(vv_buffer(dev, 15, &frame) == 0);
    assert(frame.data[4] && frame.len[4] == 128);
    vv_close(dev);
    returned_count = VIDEO_MAX_FRAME + 1;
    assert(vv_open("/dev/null", 1, 0, 0, v4l2_fourcc('S','D','U','Y'),
                   0, &dev, &layout, &mode) == -EPROTO);
    assert(dev == NULL);
    return 0;
}

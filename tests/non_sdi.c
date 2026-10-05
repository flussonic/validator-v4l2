// SPDX-License-Identifier: MIT
/* A single-plane node on a device whose other nodes can be multiplanar. */
#include <errno.h>
#include <linux/videodev2.h>
#include <stdarg.h>
#include <string.h>

int ioctl(int fd, unsigned long request, ...)
{
    (void)fd;
    if (request != VIDIOC_QUERYCAP) {
        errno = ENOTTY;
        return -1;
    }
    va_list args;
    va_start(args, request);
    struct v4l2_capability *cap = va_arg(args, struct v4l2_capability *);
    va_end(args);
    memset(cap, 0, sizeof(*cap));
    cap->capabilities = V4L2_CAP_DEVICE_CAPS | V4L2_CAP_VIDEO_CAPTURE_MPLANE;
    cap->device_caps = V4L2_CAP_VIDEO_CAPTURE | V4L2_CAP_STREAMING;
    return 0;
}

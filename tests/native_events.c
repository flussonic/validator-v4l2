// SPDX-License-Identifier: MIT
/* Exercise the actual dequeue adapter with event-only wakeups and a race. */
#include <assert.h>
#include <stdarg.h>
#define poll fake_poll
#define ioctl fake_ioctl
#define clock_gettime fake_clock_gettime
#include "../native/v4l2.c"
#undef poll
#undef ioctl
#undef clock_gettime

static unsigned polls, dequeues, event_reads, ticks;
static int events_only;
int fake_clock_gettime(clockid_t clock, struct timespec *ts)
{
    assert(clock == CLOCK_MONOTONIC);
    ts->tv_sec = 1;
    ts->tv_nsec = ticks++ * 1000000;
    return 0;
}
int fake_poll(struct pollfd *fds, nfds_t count, int timeout)
{
    assert(count == 1 && timeout > 0);
    fds[0].revents = events_only || polls++ == 0 ? POLLPRI : POLLIN;
    return 1;
}
int fake_ioctl(int fd, unsigned long request, ...)
{
    va_list args;
    va_start(args, request);
    void *arg = va_arg(args, void *);
    va_end(args);
    assert(fd == 123);
    if (request == VIDIOC_DQEVENT) {
        if (event_reads++ == 0) return 0;
    } else if (request == VIDIOC_DQBUF) {
        if (dequeues++ != 0) {
            struct v4l2_buffer *buf = arg;
            buf->index = 0;
            buf->sequence = 7;
            return 0;
        }
    } else assert(0);
    errno = EAGAIN;
    return -1;
}
int main(void)
{
    struct device dev = {.fd = 123, .type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE,
                         .memory = V4L2_MEMORY_MMAP, .count = 1};
    struct frame frame;
    assert(vv_next(&dev, 100, &frame) == 0);
    assert(frame.sequence == 7 && frame.events == 1);
    assert(polls == 3 && dequeues == 2);
    events_only = 1;
    assert(vv_next(&dev, 10, &frame) == -ETIMEDOUT);
    return 0;
}

// Standalone diagnostic for NVIDIA GBM -> EGLImage -> GLES FBO renderability.
// Build: cc -O2 -Wall -Wextra tools/egl_dmabuf_fbo_test.c -o egl_dmabuf_fbo_test \
//   -lgbm -lEGL -lGLESv2
// Run:   ./egl_dmabuf_fbo_test /dev/dri/renderD129
//
// This intentionally tests the exact Linear ABGR8888 (DRM_FORMAT_ABGR8888 /
// Chromium FourCC AB24) dma-buf path without Chromium or ANGLE SharedImages.

#define _GNU_SOURCE
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GLES2/gl2.h>
#include <GLES2/gl2ext.h>
#include <gbm.h>
#include <libdrm/drm_fourcc.h>

#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#ifndef EGL_PLATFORM_GBM_KHR
#define EGL_PLATFORM_GBM_KHR 0x31D7
#endif
#ifndef EGL_LINUX_DRM_FOURCC_EXT
#define EGL_LINUX_DRM_FOURCC_EXT 0x3271
#endif
#ifndef EGL_DMA_BUF_PLANE0_FD_EXT
#define EGL_DMA_BUF_PLANE0_FD_EXT 0x3272
#endif
#ifndef EGL_DMA_BUF_PLANE0_OFFSET_EXT
#define EGL_DMA_BUF_PLANE0_OFFSET_EXT 0x3273
#endif
#ifndef EGL_DMA_BUF_PLANE0_PITCH_EXT
#define EGL_DMA_BUF_PLANE0_PITCH_EXT 0x3274
#endif
#ifndef EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT
#define EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT 0x3443
#endif
#ifndef EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT
#define EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT 0x3444
#endif

static void print_egl_error(const char *where) {
  fprintf(stderr, "%s: EGL error=0x%04x\n", where, eglGetError());
}

static int has_extension(const char *extensions, const char *name) {
  if (!extensions || !name || !*name) return 0;
  size_t n = strlen(name);
  const char *p = extensions;
  while ((p = strstr(p, name)) != NULL) {
    if ((p == extensions || p[-1] == ' ') &&
        (p[n] == '\0' || p[n] == ' '))
      return 1;
    p += n;
  }
  return 0;
}

int main(int argc, char **argv) {
  const char *node = argc > 1 ? argv[1] : "/dev/dri/renderD129";
  const int width = 334, height = 170;
  int drm_fd = open(node, O_RDWR | O_CLOEXEC);
  if (drm_fd < 0) {
    fprintf(stderr, "open(%s): %s\n", node, strerror(errno));
    return 1;
  }

  struct gbm_device *gbm = gbm_create_device(drm_fd);
  if (!gbm) {
    fprintf(stderr, "gbm_create_device failed\n");
    close(drm_fd);
    return 1;
  }
  fprintf(stderr, "GBM device opened: format=AB24 size=%dx%d modifier=LINEAR\n",
          width, height);

  const uint64_t linear = DRM_FORMAT_MOD_LINEAR;
  struct gbm_bo *bo =
      gbm_bo_create_with_modifiers(gbm, width, height,
                                   DRM_FORMAT_ABGR8888, &linear, 1);
  if (!bo) {
    fprintf(stderr, "gbm_bo_create_with_modifiers(AB24, LINEAR) failed: %s\n",
            strerror(errno));
    gbm_device_destroy(gbm);
    close(drm_fd);
    return 2;
  }

  uint64_t actual_modifier = gbm_bo_get_modifier(bo);
  int plane_fd = gbm_bo_get_fd_for_plane(bo, 0);
  uint32_t stride = gbm_bo_get_stride_for_plane(bo, 0);
  uint32_t offset = gbm_bo_get_offset(bo, 0);
  fprintf(stderr,
          "GBM BO: modifier=0x%016llx plane_fd=%d stride=%u offset=%u\n",
          (unsigned long long)actual_modifier, plane_fd, stride, offset);
  if (plane_fd < 0) {
    fprintf(stderr, "gbm_bo_get_fd_for_plane failed: %s\n", strerror(errno));
    gbm_bo_destroy(bo);
    gbm_device_destroy(gbm);
    close(drm_fd);
    return 3;
  }

  PFNEGLGETPLATFORMDISPLAYEXTPROC get_platform_display =
      (PFNEGLGETPLATFORMDISPLAYEXTPROC)eglGetProcAddress(
          "eglGetPlatformDisplayEXT");
  if (!get_platform_display) {
    fprintf(stderr, "eglGetPlatformDisplayEXT unavailable\n");
    close(plane_fd);
    gbm_bo_destroy(bo);
    gbm_device_destroy(gbm);
    close(drm_fd);
    return 4;
  }

  EGLDisplay display =
      get_platform_display(EGL_PLATFORM_GBM_KHR, gbm, NULL);
  if (display == EGL_NO_DISPLAY) {
    print_egl_error("eglGetPlatformDisplayEXT(GBM)");
    close(plane_fd);
    gbm_bo_destroy(bo);
    gbm_device_destroy(gbm);
    close(drm_fd);
    return 5;
  }

  EGLint major = 0, minor = 0;
  if (!eglInitialize(display, &major, &minor)) {
    print_egl_error("eglInitialize");
    close(plane_fd);
    gbm_bo_destroy(bo);
    gbm_device_destroy(gbm);
    close(drm_fd);
    return 6;
  }
  fprintf(stderr, "EGL version: %d.%d\n", major, minor);

  const char *egl_ext = eglQueryString(display, EGL_EXTENSIONS);
  fprintf(stderr, "EGL_EXT_image_dma_buf_import: %s\n",
          has_extension(egl_ext, "EGL_EXT_image_dma_buf_import") ? "yes" : "no");
  fprintf(stderr, "EGL_EXT_image_dma_buf_import_modifiers: %s\n",
          has_extension(egl_ext, "EGL_EXT_image_dma_buf_import_modifiers")
              ? "yes" : "no");

  if (!has_extension(egl_ext, "EGL_EXT_image_dma_buf_import") ||
      !has_extension(egl_ext, "EGL_EXT_image_dma_buf_import_modifiers")) {
    fprintf(stderr, "Required explicit-modifier EGL import extensions missing\n");
    eglTerminate(display);
    close(plane_fd);
    gbm_bo_destroy(bo);
    gbm_device_destroy(gbm);
    close(drm_fd);
    return 7;
  }

  if (!eglBindAPI(EGL_OPENGL_ES_API)) {
    print_egl_error("eglBindAPI");
    return 8;
  }

  const EGLint config_attrs[] = {
      EGL_SURFACE_TYPE, EGL_PBUFFER_BIT,
      EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT,
      EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8, EGL_ALPHA_SIZE, 8,
      EGL_NONE};
  EGLConfig config;
  EGLint config_count = 0;
  if (!eglChooseConfig(display, config_attrs, &config, 1, &config_count) ||
      config_count < 1) {
    print_egl_error("eglChooseConfig");
    return 9;
  }

  const EGLint pbuffer_attrs[] = {EGL_WIDTH, 1, EGL_HEIGHT, 1, EGL_NONE};
  EGLSurface surface = eglCreatePbufferSurface(display, config, pbuffer_attrs);
  if (surface == EGL_NO_SURFACE) {
    print_egl_error("eglCreatePbufferSurface");
    return 10;
  }

  const EGLint context_attrs[] = {EGL_CONTEXT_CLIENT_VERSION, 2, EGL_NONE};
  EGLContext context =
      eglCreateContext(display, config, EGL_NO_CONTEXT, context_attrs);
  if (context == EGL_NO_CONTEXT) {
    print_egl_error("eglCreateContext");
    return 11;
  }
  if (!eglMakeCurrent(display, surface, surface, context)) {
    print_egl_error("eglMakeCurrent");
    return 12;
  }

  PFNEGLCREATEIMAGEKHRPROC create_image =
      (PFNEGLCREATEIMAGEKHRPROC)eglGetProcAddress("eglCreateImageKHR");
  PFNEGLDESTROYIMAGEKHRPROC destroy_image =
      (PFNEGLDESTROYIMAGEKHRPROC)eglGetProcAddress("eglDestroyImageKHR");
  PFNGLEGLIMAGETARGETTEXTURE2DOESPROC image_target =
      (PFNGLEGLIMAGETARGETTEXTURE2DOESPROC)eglGetProcAddress(
          "glEGLImageTargetTexture2DOES");
  if (!create_image || !destroy_image || !image_target) {
    fprintf(stderr, "EGLImage / GL image target entry points unavailable\n");
    return 13;
  }

  fprintf(stderr, "GL vendor: %s\n", (const char *)glGetString(GL_VENDOR));
  fprintf(stderr, "GL renderer: %s\n", (const char *)glGetString(GL_RENDERER));
  fprintf(stderr, "GL version: %s\n", (const char *)glGetString(GL_VERSION));

  const EGLint image_attrs[] = {
      EGL_WIDTH, width,
      EGL_HEIGHT, height,
      EGL_LINUX_DRM_FOURCC_EXT, DRM_FORMAT_ABGR8888,
      EGL_DMA_BUF_PLANE0_FD_EXT, plane_fd,
      EGL_DMA_BUF_PLANE0_OFFSET_EXT, (EGLint)offset,
      EGL_DMA_BUF_PLANE0_PITCH_EXT, (EGLint)stride,
      EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT, (EGLint)(uint32_t)actual_modifier,
      EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT, (EGLint)(uint32_t)(actual_modifier >> 32),
      EGL_NONE};
  EGLImageKHR image = create_image(display, EGL_NO_CONTEXT,
                                   EGL_LINUX_DMA_BUF_EXT, NULL, image_attrs);
  if (image == EGL_NO_IMAGE_KHR) {
    print_egl_error("eglCreateImageKHR(AB24 Linear)");
    return 14;
  }
  fprintf(stderr, "eglCreateImageKHR: OK\n");

  GLuint texture = 0, fbo = 0;
  glGenTextures(1, &texture);
  glBindTexture(GL_TEXTURE_2D, texture);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_NEAREST);
  glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_NEAREST);
  image_target(GL_TEXTURE_2D, image);
  GLenum gl_error = glGetError();
  fprintf(stderr, "glEGLImageTargetTexture2DOES: glGetError=0x%04x\n",
          gl_error);

  glGenFramebuffers(1, &fbo);
  glBindFramebuffer(GL_FRAMEBUFFER, fbo);
  glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0,
                         GL_TEXTURE_2D, texture, 0);
  gl_error = glGetError();
  GLenum status = glCheckFramebufferStatus(GL_FRAMEBUFFER);
  fprintf(stderr,
          "DMA-BUF texture FBO: status=0x%04x glGetError=0x%04x %s\n",
          status, gl_error,
          status == GL_FRAMEBUFFER_COMPLETE ? "COMPLETE" : "INCOMPLETE");

  const GLenum dmabuf_fbo_status = status;

  // Control: a regular driver-allocated texture should be framebuffer-renderable.
  GLuint control_texture = 0, control_fbo = 0;
  glGenTextures(1, &control_texture);
  glBindTexture(GL_TEXTURE_2D, control_texture);
  glTexImage2D(GL_TEXTURE_2D, 0, GL_RGBA, width, height, 0, GL_RGBA,
               GL_UNSIGNED_BYTE, NULL);
  glGenFramebuffers(1, &control_fbo);
  glBindFramebuffer(GL_FRAMEBUFFER, control_fbo);
  glFramebufferTexture2D(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0,
                         GL_TEXTURE_2D, control_texture, 0);
  gl_error = glGetError();
  status = glCheckFramebufferStatus(GL_FRAMEBUFFER);
  fprintf(stderr,
          "Regular GL texture FBO: status=0x%04x glGetError=0x%04x %s\n",
          status, gl_error,
          status == GL_FRAMEBUFFER_COMPLETE ? "COMPLETE" : "INCOMPLETE");

  glDeleteFramebuffers(1, &control_fbo);
  glDeleteTextures(1, &control_texture);
  glDeleteFramebuffers(1, &fbo);
  glDeleteTextures(1, &texture);
  destroy_image(display, image);
  eglMakeCurrent(display, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
  eglDestroyContext(display, context);
  eglDestroySurface(display, surface);
  eglTerminate(display);
  close(plane_fd);
  gbm_bo_destroy(bo);
  gbm_device_destroy(gbm);
  close(drm_fd);
  return dmabuf_fbo_status == GL_FRAMEBUFFER_COMPLETE ? 0 : 15;
}

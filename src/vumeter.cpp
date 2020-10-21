#include <algorithm>
#include <iostream>
#include <cassert>
#include <chrono>
#include <mutex>
#include <thread>
#include <vector>

#include <pulse/simple.h>
#include <pulse/error.h>

#include <GL/glew.h>
#include <GLFW/glfw3.h>
#define NANOVG_GLEW
#include "nanovg.h"
#define NANOVG_GL2_IMPLEMENTATION
#include "nanovg_gl.h"
#include "nanovg_gl_utils.h"

// #define NANOSVG_IMPLEMENTATION
// #include "nanosvg.h"

#include <svg.hpp>

// // memory element (can be imagined as capacitor)
// c = 0
// kCharge = 0.1
// kDischarge = 0.001

// for each sample x:
// {
//   // rectify the signal
//   x1 = abs(x)

//   if x1 > c then
//   {
//     // charge
//     c = c * (1-kCharge) + x1*kCharge
//   } else
//   {
//     // discharge
//     c = c * (1-kDischarge)
//   }
// }

const int VECTOR_SIZE = 44100 * 0.3;

std::mutex mutex;

int premult = 0;
bool exitRequested = false;
std::vector<float> vector(VECTOR_SIZE);

void AudioThread() {
  static const pa_sample_spec ss = {
    .format = PA_SAMPLE_FLOAT32,
    .rate = 44100,
    .channels = 2
  };
  pa_simple *s = NULL;
  int error;
  if (!(s = pa_simple_new(NULL, "Vu Meter", PA_STREAM_RECORD, NULL, "record", &ss, NULL, NULL, &error))) {
    std::cout << "pa_simple_new() failed: " << pa_strerror(error) << std::endl;
    pthread_exit(NULL);
  }

  for (;;) {
    if (exitRequested) { break; }
    float buf[1024];
    pa_simple_read(s, buf, sizeof(buf), &error);

    std::vector<float> vec(1024);
    vec.assign(buf, std::end(buf));

    mutex.lock();
    vector.insert(vector.end(), vec.begin(), vec.end());
    if (vector.size() > VECTOR_SIZE) {
      std::rotate(vector.begin(), vector.begin() + (vector.size() - VECTOR_SIZE), vector.end());
      vector.resize(VECTOR_SIZE);
    }
    mutex.unlock();
  }
}
int main(int argc, char** argv) {
  GLFWwindow* window;
  struct NVGcontext* vg;
  double lasttime = 0;

  NSVGimage* g_image = nsvgParseFromFile("res/Sifam_Type_32A_DIN_scale_PPM_curves.svg", "px", 96.0f);

  if (!glfwInit()) {
    std::cout << "Failed to init GLFW." << std::endl;
    return -1;
  }
  glfwWindowHint(GLFW_CONTEXT_VERSION_MAJOR, 2);
  glfwWindowHint(GLFW_CONTEXT_VERSION_MINOR, 0);
  window = glfwCreateWindow(640, 254, "Vu", NULL, NULL);
  if (!window) {
    glfwTerminate();
    return -1;
  }
  glfwMakeContextCurrent(window);
  if(glewInit() != GLEW_OK) {
    std::cout << "Could not init glew." << std::endl;
    return -1;
  }

  vg = nvgCreateGL2(NVG_ANTIALIAS | NVG_STENCIL_STROKES);

  if (vg == NULL) {
    std::cout << "Could not init nanovg." << std::endl;
    return -1;
  }
  glfwSwapInterval(0);
  glfwSetTime(0);
  lasttime = glfwGetTime();

  std::thread audioThread(AudioThread);
  audioThread.detach();

  float lastRotationLeft = -1.0;
  float lastRotationRight = -1.0;

  while (!glfwWindowShouldClose(window)) {
    int winWidth, winHeight;
    int fbWidth, fbHeight;
    float pxRatio;

    float leftPeak = 0.0;
    float rightPeak = 0.0;

    float leftSum = 0.0;
    float rightSum = 0.0;

    mutex.lock();
    for (int i = 0; i < VECTOR_SIZE - 1; i = i + 2) {
      leftSum += std::pow(vector[i], 2);
      rightSum += std::pow(vector[i + 1], 2);
    }
    // std::cout << std::abs(vector[0]) << std::endl;
    mutex.unlock();

    leftPeak = 1.0 + std::log10(std::sqrt(leftSum / (VECTOR_SIZE / 2)));
    rightPeak = 1.0 + std::log10(std::sqrt(rightSum / (VECTOR_SIZE / 2)));

    glfwGetWindowSize(window, &winWidth, &winHeight);
    glfwGetFramebufferSize(window, &fbWidth, &fbHeight);
    pxRatio = (float)fbWidth / (float)winWidth;

    glViewport(0, 0, fbWidth, fbHeight);
    if (premult) {
      glClearColor(0,0,0,0);
    } else {
      glClearColor(0.3f, 0.3f, 0.32f, 1.0f);
    }
    glClear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT | GL_STENCIL_BUFFER_BIT);

    float rangeRadians = 47 * M_PI/180;

    nvgBeginFrame(vg, winWidth, winHeight, pxRatio);

    drawSVG(vg, g_image);

    // Marks
    // for (int dB = 0; dB > -96; dB -= 6) {
    //   nvgSave(vg);

    //   float value = std::pow(2, dB / 6);
    //   float power = std::pow(value, 2);
    //   float peak = 1.0 + std::log10(std::sqrt(power));

    //   float rot = rangeRadians * peak;
    //   nvgTranslate(vg, 160, 207);
    //   nvgRotate(vg, rot);
    //   nvgBeginPath(vg);
    //   nvgRect(vg, -2, -180, 4, 4);
    //   nvgFillColor(vg, nvgRGBA(200,40,50,255));
    //   nvgFill(vg);
    //   nvgRestore(vg);
    // }

    nvgSave(vg);

    float rotation = rangeRadians * leftPeak;
    rotation = std::min(rangeRadians, std::max(-rangeRadians, rotation));
    nvgTranslate(vg, 160, 207);
    nvgRotate(vg, rotation);
    nvgBeginPath(vg);
    nvgRect(vg, -1, -172, 2, 172);
    nvgFillColor(vg, nvgRGBA(255,255,255,255));
    nvgFill(vg);

    // Motion blur
    nvgBeginPath(vg);
    nvgMoveTo(vg, 0, 0);
    nvgLineTo(vg, 0, -172);
    nvgRotate(vg, lastRotationLeft - rotation);
    nvgLineTo(vg, 0, -172);
    nvgClosePath(vg);
    nvgFillColor(vg, nvgRGBA(255,255,255,127));
    nvgFill(vg);
    nvgRestore(vg);

    lastRotationLeft = rotation;

    nvgSave(vg);
    nvgTranslate(vg, 320, 0);
    drawSVG(vg, g_image);

    rotation = rangeRadians * rightPeak;
    rotation = std::min(rangeRadians, std::max(-rangeRadians, rotation));
    nvgTranslate(vg, 160, 207);
    nvgRotate(vg, rotation);
    nvgBeginPath(vg);
    nvgRect(vg, -1, -172, 2, 172);
    nvgFillColor(vg, nvgRGBA(255,255,255,255));
    nvgFill(vg);

    // Motion blur
    nvgBeginPath(vg);
    nvgMoveTo(vg, 0, 0);
    nvgLineTo(vg, 0, -172);
    nvgRotate(vg, lastRotationRight - rotation);
    nvgLineTo(vg, 0, -172);
    nvgClosePath(vg);
    nvgFillColor(vg, nvgRGBA(255,255,255,127));
    nvgFill(vg);
    nvgRestore(vg);

    lastRotationRight = rotation;

    nvgRestore(vg);

    nvgEndFrame(vg);

    glfwSwapBuffers(window);
    glfwPollEvents();

    double now = glfwGetTime();
    double drawingTime = now - lasttime;
    int target = 1000 / 60;
    std::this_thread::sleep_for(
      std::chrono::milliseconds(target - (int)(drawingTime * 1000))
    );
    lasttime = glfwGetTime();
  }

  exitRequested = true;
  // audioThread.join();

  return 0;
}

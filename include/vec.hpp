// This code is taken from VCV Rack
#pragma once
#include <math.h> // for global namespace functions
#include <cmath> // for std::isfinite, etc
#include <cstdlib> // for std::abs, etc


// Use a few standard math functions without std::
using std::isfinite;
using std::isinf;
using std::isnan;
using std::isnormal;

////////////////////
// basic integer functions
////////////////////

/** Returns true if x is odd */
inline bool isOdd(int x) {
	return x % 2 != 0;
}

/** Returns true if x is odd */
inline bool isEven(int x) {
	return x % 2 == 0;
}

/** Returns the minimum of `a` and `b` */
inline int min(int a, int b) {
	return (a < b) ? a : b;
}

/** Returns the maximum of `a` and `b` */
inline int max(int a, int b) {
	return (a > b) ? a : b;
}

/** Limits `x` between `a` and `b`
Assumes a <= b
*/
inline int clamp(int x, int a, int b) {
	return min(max(x, a), b);
}

/** Limits `x` between `a` and `b`
If a > b, switches the two values
*/
inline int clamp2(int x, int a, int b) {
	return clamp(x, min(a, b), max(a, b));
}

/** Euclidean modulus, always returns 0 <= mod < base for positive base.
*/
inline int eucmod(int a, int base) {
	int mod = a % base;
	return (mod >= 0) ? mod : mod + base;
}

/** Returns floor(log_2(n)), or 0 if n == 1.
*/
inline int log2(int n) {
	int i = 0;
	while (n >>= 1) {
		i++;
	}
	return i;
}

inline bool ispow2(int n) {
	return n > 0 && (n & (n - 1)) == 0;
}

////////////////////
// basic float functions
////////////////////

/** Returns the minimum of `a` and `b` */
inline float min(float a, float b) {
	return (a < b) ? a : b;
}

/** Returns the maximum of `a` and `b` */
inline float max(float a, float b) {
	return (a > b) ? a : b;
}

/** Limits `x` between `a` and `b`
Assumes a <= b
*/
inline float clamp(float x, float a, float b) {
	return min(max(x, a), b);
}

/** Limits `x` between `a` and `b`
If a > b, switches the two values
*/
inline float clamp2(float x, float a, float b) {
	return clamp(x, min(a, b), max(a, b));
}

/** Returns 1.f for positive numbers and -1.f for negative numbers (including positive/negative zero) */
inline float sgn(float x) {
	return copysignf(1.0f, x);
}

inline float eucmod(float a, float base) {
	float mod = fmodf(a, base);
	return (mod >= 0.0f) ? mod : mod + base;
}

inline bool isNear(float a, float b, float epsilon = 1.0e-6f) {
	return fabsf(a - b) <= epsilon;
}

/** If the magnitude of x if less than eps, return 0 */
inline float chop(float x, float eps) {
	return (-eps < x && x < eps) ? 0.0f : x;
}

inline float rescale(float x, float a, float b, float yMin, float yMax) {
	return yMin + (x - a) / (b - a) * (yMax - yMin);
}

inline float crossfade(float a, float b, float frac) {
	return a + frac * (b - a);
}

/** Linearly interpolate an array `p` with index `x`
Assumes that the array at `p` is of length at least floor(x)+1.
*/
inline float interpolateLinear(const float *p, float x) {
	int xi = x;
	float xf = x - xi;
	return crossfade(p[xi], p[xi+1], xf);
}

/** Complex multiply c = a * b
Arguments may be the same pointers
i.e. cmultf(&ar, &ai, ar, ai, br, bi)
*/
inline void cmult(float *cr, float *ci, float ar, float ai, float br, float bi) {
	*cr = ar * br - ai * bi;
	*ci = ar * bi + ai * br;
}

////////////////////
// 2D vector
////////////////////

struct Vec {
	float x = 0.f;
	float y = 0.f;

	Vec() {}
	Vec(float x, float y) : x(x), y(y) {}

	Vec neg() {
		return Vec(-x, -y);
	}
	Vec plus(Vec b) {
		return Vec(x + b.x, y + b.y);
	}
	Vec minus(Vec b) {
		return Vec(x - b.x, y - b.y);
	}
	Vec mult(float s) {
		return Vec(x * s, y * s);
	}
	Vec mult(Vec b) {
		return Vec(x * b.x, y * b.y);
	}
	Vec div(float s) {
		return Vec(x / s, y / s);
	}
	Vec div(Vec b) {
		return Vec(x / b.x, y / b.y);
	}
	float dot(Vec b) {
		return x * b.x + y * b.y;
	}
	float norm() {
		return hypotf(x, y);
	}
	Vec flip() {
		return Vec(y, x);
	}
	Vec min(Vec b) {
		return Vec(std::fmin(x, b.x), std::fmin(y, b.y));
	}
	Vec max(Vec b) {
		return Vec(std::fmax(x, b.x), std::fmax(y, b.y));
	}
	Vec round() {
		return Vec(roundf(x), roundf(y));
	}
	Vec floor() {
		return Vec(floorf(x), floorf(y));
	}
	Vec ceil() {
		return Vec(ceilf(x), ceilf(y));
	}
	bool isEqual(Vec b) {
		return x == b.x && y == b.y;
	}
	bool isZero() {
		return x == 0.0f && y == 0.0f;
	}
	bool isFinite() {
		return isfinite(x) && isfinite(y);
	}
};


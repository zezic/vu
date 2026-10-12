# Audio measurements and meter ballistics

Research notes, 2026-10-12. Measurement, detector timing, calibration, and display motion are separate choices. The familiar meter name usually specifies a combination of them.

## Analog programme meters

A conventional VU meter measures rectified average level, approximately, rather than true RMS. ITU's historical comparison gives its rectifier exponent as 1.0-1.4, versus 2 for mean-square detection. It reaches 99% of its final reading in 300 ms; that is a response requirement, not a 300 ms rectangular averaging window. Its rise and return are comparable. A physical moving-coil movement contributes to the measured response. [ITU-R BS.645-2, Annex 2](https://www.itu.int/dms_pubrec/itu-r/rec/bs/R-REC-BS.645-2-199203-I!!PDF-E.pdf)

Quasi-peak programme meters deliberately under-read very short peaks and retain longer ones for readability:

| Family | Integration time | Return specification |
| --- | --- | --- |
| German/DIN, IEC Type I | 5 ms | 20 dB in 1.7 s |
| BBC, IEC Type IIa | 10 ms | 26 dB in 3 s |
| EBU, IEC Type IIb | 10 ms | 24 dB in 2.8 s |

Here integration means a tone burst reaches approximately 2 dB below the sustained reading. These numbers are neither averaging-window lengths nor exponential time constants. [ITU-R BS.645-2, Annex 2](https://www.itu.int/dms_pubrec/itu-r/rec/bs/R-REC-BS.645-2-199203-I!!PDF-E.pdf)

The original BBC investigation explains why return is intentionally slow: operators need readable peaks; excessively rapid movement tires the observer. [BBC Research Report 1958-29, section 8](https://downloads.bbc.co.uk/rd/pubs/reports/1958-29.pdf)

The physical PPM movement is substantially slower than its 10 ms detector integration. Sifam's manufacturer datasheet compares a full-specification PPM movement with its Vintage Fast Response movement. The nominal full-specification curve reaches approximately 56% deflection at 50 ms, 99% at 100 ms, peaks around 105.5% at 140 ms, and returns close to equilibrium by 250 ms. These are approximate readings from the published graph, whose axes are percent deflection and seconds. The text identifies a DC milliammeter, 1 mA full-scale current, 600 ohm nominal terminal resistance for the full-specification PPM, and individual adjustment of each full-specification movement's ballistics. It does not give a separate detailed excitation protocol for the plotted response. [Sifam Vintage Fast Response datasheet, manufacturer document mirrored by Don Audio, page 1](https://www.don-audio.com/mediafiles/datasheets/Sifam-Vintage_FR-Meters.pdf)

A second-order movement with damping ratio approximately 0.68 and natural frequency approximately 5 Hz reproduces that nominal curve: about 57% at 50 ms, 100% at 100 ms, and 5.4% overshoot near 136 ms. These parameters are an inferred fit to the graph, not values published by Sifam. They describe the movement separately from the quasi-peak detector's attack and return. This is a practical physical reference, rather than an IEC compliance claim or a model of every PPM movement.

The blue Vintage FR curve has approximately 28% deflection at 50 ms, 68% at
100 ms, 91% at 150 ms, and full deflection near 200 ms. Its overswing is roughly
2.5% near 240 ms, settling by about 400 ms. A least-squares fit to digitized
points gives approximately 3.07 Hz natural frequency and 0.763 damping ratio.
This second-order approximation follows the blue trace within about 2 percentage
points at the sampled times. These are inferred parameters from the same Sifam
graph, not manufacturer-published component values.

The complete VU standard specifies electromechanical instrument performance and testing. Its full text was not available in this research, so exact overshoot tolerances and a compliant movement model remain to be verified there. Nordic-specific timing was also not independently verified. [IEC 60268-17](https://webstore.iec.ch/en/publication/1216)

## RMS and digital peaks

RMS is an energy measurement: `sqrt(mean(x*x))`. The averaging kernel is part of its definition in an implementation: rectangular moving windows and exponential averaging give different responses. A generic RMS detector does not inherently prescribe a mechanical needle response.

Sample peak takes the largest absolute sample. True peak estimates the maximum of the reconstructed continuous waveform, which can exceed every stored sample. ITU supplies an oversampled interpolation method; its example uses 4x oversampling at 48 kHz. Peak hold and display decay are additional presentation decisions, not an RMS integration stage. [ITU-R BS.1770-5, Annex 2](https://www.itu.int/dms_pubrec/itu-r/rec/bs/R-REC-BS.1770-5-202311-I!!PDF-E.pdf)

## Loudness

Programme loudness uses K-weighted mean-square measurements with channel weighting, rather than plain per-channel RMS. Integrated loudness includes gating. [ITU-R BS.1770-5, Annex 1](https://www.itu.int/dms_pubrec/itu-r/rec/bs/R-REC-BS.1770-5-202311-I!!PDF-E.pdf)

EBU Mode specifies 400 ms rectangular windows for momentary loudness and 3 s rectangular windows for short-term loudness, both ungated; integrated loudness covers the selected programme with gating. It explicitly forbids additional attack/release slowdown after these windows. An inertial needle changing the displayed loudness response would therefore conflict with that requirement. [EBU Tech 3341, section 2.2](https://tech.ebu.ch/docs/tech/tech3341.pdf)

## Implementation in this app

The four modes share the existing rendering, shutter exposure, overload lamps,
and stop-contact mechanics. Detector drive spans are retained at approximately
1 ms resolution and consumed in audio-time order, so a brief event between
screen refreshes still acts on the needle. Samples are not clamped to full scale.
RMS accumulates floating-point power instead of quantized integer squares.

| Mode | Detector | Movement | Units |
| --- | --- | --- | --- |
| VU | Rectified amplitude, sine calibration `pi/(2*sqrt(2))` | Underdamped; 99% step response at 300 ms, 1.25% overshoot | VU |
| PPM | Parallel quasi-peak envelopes, 26 dB / 3 s return | Sifam-style PPM movement | dBFS |
| RMS | 300 ms rectangular mean-square window | Critically damped movement | dBFS |
| Sample Peak | Absolute sample peaks, 100 ms hold, 20 dB / s return | Sifam-style PPM movement | dBFS |

The moving-coil approximation is `theta'' + 2*zeta*omega*theta' + omega^2*theta
= omega^2*drive`. VU uses `zeta = 0.8127169864286775` and
`omega = 13.51191256253271 rad/s`, solving for the chosen rise and overshoot.
Peak modes select one of two fits: Full Spec uses `zeta = 0.68` and
`omega = 2*pi*5 rad/s`; Vintage FR uses `zeta = 0.763` and
`omega = 2*pi*3.07 rad/s`. Both follow the Sifam curves described above. RMS uses critical
damping and `omega = 2*pi*4 rad/s`. Position and velocity remain continuous
when drive changes. The VU equilibrium angle is proportional to drive amplitude;
the dBFS modes use the adjustable logarithmic scale. Shutter exposure integrates
position, including reversals and overshoot, without changing motion.

Preamp is a common amplitude multiplier. Zero VU is a sine with RMS amplitude 1
after preamp; zero dBFS is amplitude 1 after preamp. A full-scale sine at unity
preamp reads approximately -3.01 dBFS in RMS mode. PPM is shown on a dBFS scale,
rather than the BBC 1-7 scale. Mode and peak movement selection persist in the existing RON config. `[N]`
switches Full Spec and Vintage FR in PPM and Sample Peak, preserving position,
velocity, bend, and past shutter segments. It changes the mechanical response;
the detector's charging, hold, and discharge behavior stays defined by the mode.
VU and RMS retain their own response parameters.

The PPM detector combines two parallel rectifier envelopes. Charging rates are
200 and 860 per second, with exponential coefficients calculated from the sample
rate. Both envelopes discharge at 26 dB per 3 seconds; their sum has a
sine-calibration factor of 0.5141. This mathematical topology and nominal
parameters are documented in [Fons Adriaensen's IEC Type II detector in
x42/meters.lv2](https://github.com/x42/meters.lv2/blob/master/jmeters/iec2ppmdsp.cc).
This implementation uses exponential per-sample coefficients and continuous
leakage rather than the reference implementation's grouped Euler updates.

Regression tests compare the physical movement against approximate points read
from the Sifam graph. They also distinguish detector integration from pointer
indication. At 44.1, 48, and 96 kHz, the detector approximates the 1.5 ms, 5 ms,
10 ms, and 100 ms tone-burst readings of -9, -4, -2, and 0 dB. The complete
physical path is checked at 15, 60, and 240 FPS using the settled sine reading
as its reference. With Full Spec and the default adjustable dB scale, its short-burst maxima
are approximately -9.1, -3.5, and -1.5 dB; a 100 ms burst can show about +1.1 dB
of pointer overswing. After removal of a settled tone, the pointer falls about
25.6 dB in 3 seconds, with the difference from the 26 dB detector discharge
coming from mechanical lag. These checks establish the modeled responses, not
full IEC conformance for every adjustable scale configuration.

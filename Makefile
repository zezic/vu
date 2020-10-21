all: src/vumeter.cpp
	g++ -std=c++11 -pthread -g -o vumeter -Idep/nanovg/src -Idep/nanovg/example -Idep/nanosvg/src -Iinclude src/vumeter.cpp dep/nanovg/src/nanovg.c dep/nanosvg/src/nanosvg.h $(shell pkg-config --libs glfw3 glew libpulse-simple)
	
static: src/vumeter.cpp
	g++ -static -static-libgcc -static-libstdc++ -std=c++11 -pthread -g -o vumeter-static -Idep/nanovg/src -Idep/nanovg/example -Idep/nanosvg/src -Iinclude src/vumeter.cpp dep/nanovg/src/nanovg.c dep/nanosvg/src/nanosvg.h $(shell pkg-config --libs glfw3 glew libpulse-simple)
	ar 

clean:
	$(RM) vumeter

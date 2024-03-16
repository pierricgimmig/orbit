#include <iostream>

#include "hijk/hijk.h"

__declspec(noinline) void MyFunction(int a, float b) {
  std::cout << "a: " << a << " b: " << b << std::endl;
}

extern "C" {

void Prolog(void* original_function, struct Hijk_PrologContext* prolog_context) {
  std::cout << "User Prolog!\n";
}
void Epilog(void* original_function, struct Hijk_EpilogContext* epilog_context) {
  std::cout << "User Epilog!\n";
}
}

int main() {
  std::cout << "hi hijk\n";

  void* target_function = &MyFunction;

  Hijk_CreateHook(target_function, &Prolog, &Epilog);
  MyFunction(1, 23.45f);

  Hijk_EnableHook(target_function);
  MyFunction(1, 23.45f);

  Hijk_DisableHook(target_function);
  MyFunction(1, 23.45f);

  Hijk_RemoveHook(target_function);
  MyFunction(1, 23.45f);

  std::cout << "bye hijk\n";
}

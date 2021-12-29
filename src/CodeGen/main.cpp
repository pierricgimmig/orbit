#include <iostream>
#include <filesystem>
#include <fstream>

int main()
{
    std::cout << "Hello world " << std::endl;
    std::filesystem::create_directories("generated/");
    {
        std::ofstream outfile("generated/blah.cpp");
        outfile << "#include <iostream>" << std::endl
                << "int main(){\n  std::cout << \"generated exe  yeah!\" << std::endl;\n}" << std::endl;
        outfile.close();
        std::cout << "generated/blah.cpp";
    }

    {
        std::ofstream outfile("generated/blah.h");
        outfile << "// blah.h" << std::endl;
        outfile.close();
        std::cout << "generated/blah.h";
    }
}
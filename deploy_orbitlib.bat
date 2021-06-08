set source_path="C:\git\orbitprofiler"
set source_bin_path= %source_path%"\build\OrbitLib\RelWithDebInfo"
set destination_path="C:\git\orbit\third_party\OrbitLib"
set destination_path_include=%destination_path%"\include\OrbitLib"

set orbit_dll_path="C:\git\orbitprofiler\build\OrbitDll\RelWithDebInfo"
set destination_orbit_bin_path="C:\git\orbit\build_default_relwithdebinfo\bin"

if not exist %destination_path% mkdir %destination_path% /Y
if not exist %destination_path_include% mkdir %destination_path_include% /Y

copy %source_bin_path%\OrbitLib.dll %destination_path%
copy %source_bin_path%\OrbitLib.lib %destination_path%
copy %source_bin_path%\OrbitLib.pdb %destination_path%
copy %source_path%\OrbitLib\OrbitLib.h %destination_path_include%

copy %source_bin_path%\OrbitLib.dll %destination_orbit_bin_path%
copy %source_bin_path%\OrbitLib.lib %destination_orbit_bin_path%
copy %source_bin_path%\OrbitLib.pdb %destination_orbit_bin_path%

copy %orbit_dll_path%\OrbitDll.dll %destination_orbit_bin_path%\Orbit64.dll
copy %orbit_dll_path%\OrbitDll.pdb %destination_orbit_bin_path%\Orbit64.pdb
set source_path="C:\git\orbitprofiler"
set bin_path= %source_path%"\build\OrbitLib\RelWithDebInfo"
set destination_path="C:\git\orbit\third_party\OrbitLib"
set destination_path_include=%destination_path%"\include\OrbitLib"

if not exist %destination_path% mkdir %destination_path% /Y
if not exist %destination_path_include% mkdir %destination_path_include% /Y

xcopy %bin_path%\OrbitLib.dll %destination_path% /Y
xcopy %bin_path%\OrbitLib.lib %destination_path% /Y
xcopy %bin_path%\OrbitLib.pdb %destination_path% /Y
xcopy %source_path%\OrbitLib\OrbitLib.h %destination_path_include% /Y
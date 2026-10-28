@echo off
rem Profilage (#41) : lance voxel_engine_profile.exe pour un scenario.
rem   profilage.bat course
rem ecrit trace-course.zip (temps par systeme, resume par
rem tools/trace_stats.py) et log-course.txt (diagnostics toutes les 5 s).
rem Quitter le jeu normalement (fermer la fenetre) : la trace est ecrite a la sortie.
cd /d "%~dp0"
set NAME=%1
if "%NAME%"=="" set NAME=session
set TRACE_CHROME=trace-%NAME%.json
set NO_COLOR=1
rem Seulement les systemes, les schedules et les diagnostics : le reste
rem (executeur de Bevy, internes du rendu) gonfle la trace pour rien.
set RUST_LOG=warn,bevy_ecs::system=info,bevy_ecs::schedule::schedule=info,bevy_diagnostic=info,bevy_render::renderer=info,voxel_engine=info,voxel_core=info
voxel_engine_profile.exe > log-%NAME%.txt 2>&1
rem La trace fait des Go ; zippee, environ 25 fois moins.
echo Compression de la trace...
tar -a -c -f trace-%NAME%.zip trace-%NAME%.json && del trace-%NAME%.json
echo Fini : trace-%NAME%.zip et log-%NAME%.txt

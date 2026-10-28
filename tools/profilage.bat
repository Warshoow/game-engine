@echo off
rem Profilage (#41) : lance voxel_engine_profile.exe pour un scenario.
rem   profilage.bat course
rem ecrit trace-course.json (temps par systeme, a ouvrir dans
rem https://ui.perfetto.dev) et log-course.txt (diagnostics toutes les 5 s).
rem Quitter le jeu normalement (fermer la fenetre) : la trace est ecrite a la sortie.
cd /d "%~dp0"
set NAME=%1
if "%NAME%"=="" set NAME=session
set TRACE_CHROME=trace-%NAME%.json
set NO_COLOR=1
rem Sans les spans de l'executeur de Bevy : 40 % de la trace, inutiles ici.
set RUST_LOG=info,wgpu=error,naga=warn,bevy_ecs::schedule::executor=warn,bevy_ecs::query=warn
voxel_engine_profile.exe > log-%NAME%.txt 2>&1
echo Fini : trace-%NAME%.json et log-%NAME%.txt

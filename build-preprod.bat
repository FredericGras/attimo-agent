@echo off
setlocal
cd /d "%~dp0"

set "ATTIMO_API_URL=https://dev-saas.attimo-gallery.com"

REM ==========================================================
REM   Agent de DEV, installe A COTE de celui de production (0.3.1)
REM
REM   - ATTIMO_EDITION=dev : memoire locale separee
REM     (%APPDATA%\com.attimo-gallery.agent.dev : sessions, file video,
REM     mot de passe d'application). Jamais celle de la production.
REM   - src-tauri\tauri.preprod.conf.json : autre nom visible
REM     ("Attimo Agent Terrain DEV"), autre identifiant, autre code
REM     d'installation Windows. L'installeur de DEV ne remplace donc
REM     jamais l'agent de production, et inversement.
REM ==========================================================
set "ATTIMO_EDITION=dev"

echo ==========================================================
echo   BUILD PREPROD (agent DEV, installe a cote de la production)
echo   Cible API : %ATTIMO_API_URL%
echo   Edition   : %ATTIMO_EDITION%
echo ==========================================================
echo.

call npm run tauri build -- --config src-tauri/tauri.preprod.conf.json
if errorlevel 1 (
    echo.
    echo *** BUILD ECHOUE ***
    pause
    exit /b 1
)

echo.
echo ---- Verification de l'URL embarquee dans le binaire ----

findstr /C:"dev-saas.attimo-gallery.com" "src-tauri\target\release\attimo-agent.exe" >nul
if errorlevel 1 (
    echo.
    echo *** ALERTE : l'URL de preprod est introuvable dans le binaire ***
    pause
    exit /b 1
)

echo ---- Verification de la memoire locale separee ----

findstr /C:"com.attimo-gallery.agent.dev" "src-tauri\target\release\attimo-agent.exe" >nul
if errorlevel 1 (
    echo.
    echo *** ALERTE : l'edition DEV est introuvable dans le binaire ***
    echo *** Il partagerait la memoire de l'agent de production : NE PAS INSTALLER ***
    pause
    exit /b 1
)

echo OK : binaire de preprod valide.
echo.
echo Le .msi se trouve dans : src-tauri\target\release\bundle\msi\
echo Son nom commence par "Attimo Agent Terrain DEV".
echo Deposer UNIQUEMENT dans /home/attimo-pp/www/public/downloads/
echo.
pause

@echo off
setlocal
cd /d "%~dp0"

set "ATTIMO_API_URL=https://app.attimo-gallery.com"

REM Production : aucune edition. Videe explicitement, au cas ou la
REM variable trainerait dans l'environnement du poste : le binaire
REM utiliserait alors la memoire locale de l'agent de DEV (0.3.1).
set "ATTIMO_EDITION="

echo ==========================================================
echo   BUILD PRODUCTION
echo   Cible API : %ATTIMO_API_URL%
echo ==========================================================
echo.

call npm run tauri build
if errorlevel 1 (
    echo.
    echo *** BUILD ECHOUE - rien a deployer ***
    pause
    exit /b 1
)

echo.
echo ---- Verification de l'URL embarquee dans le binaire ----

findstr /C:"dev-saas" "src-tauri\target\release\attimo-agent.exe" >nul
if not errorlevel 1 (
    echo.
    echo *** ALERTE : le binaire contient encore "dev-saas" ***
    echo *** NE PAS DEPLOYER EN PRODUCTION ***
    pause
    exit /b 1
)

findstr /C:"app.attimo-gallery.com" "src-tauri\target\release\attimo-agent.exe" >nul
if errorlevel 1 (
    echo.
    echo *** ALERTE : l'URL de production est introuvable dans le binaire ***
    echo *** NE PAS DEPLOYER ***
    pause
    exit /b 1
)

findstr /C:"com.attimo-gallery.agent.dev" "src-tauri\target\release\attimo-agent.exe" >nul
if not errorlevel 1 (
    echo.
    echo *** ALERTE : le binaire utilise la memoire locale de l'agent de DEV ***
    echo *** NE PAS DEPLOYER EN PRODUCTION ***
    pause
    exit /b 1
)

echo OK : binaire de production valide.
echo.
echo Le .msi se trouve dans : src-tauri\target\release\bundle\msi\
echo Prendre celui qui s'appelle "Attimo Agent Terrain_..." (sans DEV).
echo Renommer en Attimo_Agent_Terrain_international.msi
echo Deposer UNIQUEMENT dans /home/attimo/www/public/downloads/
echo.
pause

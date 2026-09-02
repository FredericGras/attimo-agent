@echo off
setlocal
cd /d "%~dp0"

set "ATTIMO_API_URL=https://dev-saas.attimo-gallery.com"

echo ==========================================================
echo   BUILD PREPROD
echo   Cible API : %ATTIMO_API_URL%
echo ==========================================================
echo.

call npm run tauri build
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

echo OK : binaire de preprod valide.
echo.
echo Le .msi se trouve dans : src-tauri\target\release\bundle\msi\
echo Deposer UNIQUEMENT dans /home/attimo-pp/www/public/downloads/
echo.
pause

@echo off
setlocal
cd /d "%~dp0"

REM ==========================================================
REM   Lancement de la suite de tests
REM
REM   ATTIMO_API_URL doit etre definie : auth.rs, uploader.rs et
REM   video_uploader.rs lisent l'URL de l'API a la COMPILATION via
REM   env!(), qui refuse de compiler si la variable est absente.
REM   Sans elle, "cargo test" echoue avant meme d'executer un test.
REM
REM   La valeur de preprod est utilisee a dessein : aucun test ne
REM   fait d'appel reseau, l'URL ne sert qu'a satisfaire env!(), et
REM   viser la preprod evite qu'un binaire de test se retrouve un
REM   jour a pointer sur la production.
REM ==========================================================

set "ATTIMO_API_URL=https://dev-saas.attimo-gallery.com"

echo ==========================================================
echo   TESTS
echo   Cible API compilee : %ATTIMO_API_URL%
echo ==========================================================
echo.

cd src-tauri

REM Les arguments sont transmis a cargo : test.bat video_queue ne
REM lance que les tests dont le nom contient "video_queue".
cargo test %*
if errorlevel 1 (
    echo.
    echo *** DES TESTS ECHOUENT ***
    pause
    exit /b 1
)

echo.
echo OK : toute la suite passe.
echo.
pause

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

REM Libelles de l'interface, depuis 0.3.3 : tests Node, si Node est installe.
cd ..
where node >nul 2>nul
if errorlevel 1 goto sans_node

node --test "tests/*.test.cjs"
if errorlevel 1 goto echec_interface
goto fin

:sans_node
echo.
echo Node.js absent : tests des libelles de l'interface non lances.
goto fin

:echec_interface
echo.
echo *** DES TESTS DE L'INTERFACE ECHOUENT ***
pause
exit /b 1

:fin
echo.
echo OK : toute la suite passe.
echo.
pause

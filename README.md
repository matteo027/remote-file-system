# remote-file-system
Rust project - remote file system

📁 API Documentation
🔐 Authentication Endpoints
POST /api/login

Authenticate a user using Passport's local strategy.

    Auth required: No

    Body parameters:

        username (string)

        password (string)

    Response:

        200 OK on success

        401 Unauthorized on failure

POST /api/logout

Logs out the currently authenticated user.

    Auth required: Yes

    Response:

        200 OK on success

GET /api/me

Returns information about the currently logged-in user.

    Auth required: Yes

    Response:

        200 OK with user info

        401 Unauthorized if not authenticated

📂 Filesystem Endpoints

All the following routes require authentication.
Directories
GET /api/directories/*

List contents of a directory.

    Example: /api/directories/home/user/docs

    Response:

        200 OK with directory listing

POST /api/directories/*

Create a new directory.

    Example: /api/directories/home/user/newFolder

    Response:

        201 Created on success

        400 Bad Request on failure

DELETE /api/directories/*

Delete an existing directory.

    Example: /api/directories/home/user/oldFolder

    Response:

        200 OK on success

        404 Not Found if directory doesn't exist

Files
POST /api/files/*

Create a new file.

    Example: /api/files/home/user/newFile.txt

    Response:

        201 Created on success

PUT /api/files/*

Write to a file (overwrite).

    Example: /api/files/home/user/file.txt

    Body: Raw file content

    Response:

        200 OK on success

GET /api/files/*

Read a file.

    Example: /api/files/home/user/file.txt

    Response:

        200 OK with file contents

DELETE /api/files/*

Delete a file.

    Example: /api/files/home/user/file.txt

    Response:

        200 OK on success

        404 Not Found if file doesn't exist

PUT /api/files/* (Rename)

Rename a file.

    Note: This route may conflict with the PUT above (write). Clarify in implementation.

    Body parameters:

        newName (string)

    Response:

        200 OK on success

File Metadata
PUT /api/mod/*

Modify file attributes (e.g., permissions, timestamps).

    Example: /api/mod/home/user/file.txt

    Body parameters: (depends on your implementation)

    Response:

        200 OK on success

⚠️ Notes

    All routes under /api/files/*, /api/directories/*, and /api/mod/* require the user to be authenticated via middleware (isLoggedIn).

    The route PUT /api/files/* appears to serve both writing and renaming, which may lead to conflict unless method differentiation is handled (e.g., via headers or request body).
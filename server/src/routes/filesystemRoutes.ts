import { Router } from 'express';
import express from 'express';
import { FileController } from '../controllers/fileController';
import { ReadWriteController } from '../controllers/RWController';
import { AttributeController } from '../controllers/attrController';
import { Express } from 'express-serve-static-core';
import { AuthenticationController } from '../controllers/authenticationController';

const router = Router();
const fileController = new FileController();
const rwController = new ReadWriteController();
const attrController = new AttributeController();
const isLoggedIn = (new AuthenticationController).isLoggedIn;

export function setRoutes(app: Express) {
    app.use('/', router);

    router.get('/api/files/:ino/attributes', isLoggedIn, attrController.getattr);
    router.patch('/api/files/:ino/attributes', isLoggedIn, attrController.setattr);

    router.get('/api/directories/:parentIno/entries/:name/lookup', isLoggedIn, attrController.lookup);    
    router.get('/api/directories/:ino/entries', isLoggedIn, attrController.readdir);

    router.post('/api/directories/:parentIno/dirs/:name', isLoggedIn, fileController.mkdir);
    router.delete('/api/directories/:parentIno/dirs/:name', isLoggedIn, fileController.rmdir);
    router.post('/api/directories/:parentIno/files/:name', isLoggedIn, fileController.create);
    router.delete('/api/directories/:parentIno/files/:name', isLoggedIn, fileController.unlink);

    router.patch('/api/directories/:oldParentIno/entries/:oldName', isLoggedIn, fileController.rename); // rename

    router.put('/api/files/stream/:ino', isLoggedIn, rwController.writeStream);
    router.get('/api/files/stream/:ino', isLoggedIn, rwController.readStream);
    router.put('/api/files/:ino', isLoggedIn, express.raw({type:'application/octet-stream', limit: '1gb'}), rwController.write);
    router.get('/api/files/:ino', isLoggedIn, rwController.read);

    router.post('/api/links/:targetIno', isLoggedIn, fileController.hardlink);
    router.post('/api/symlinks', isLoggedIn, fileController.symlink);
    router.get('/api/symlinks/:ino', isLoggedIn, fileController.readlink);
    
}